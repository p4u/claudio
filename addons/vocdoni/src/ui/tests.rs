//! The Vocdoni checklists' tests: a plugin's setup as a checklist, saved
//! and deferred through the runner, and offered once at first sight.

use std::sync::Arc;

use super::*;
use crate::plugins::choices::{self, PluginChoices};
use crate::plugins::state::PluginsFetch;
use crate::proxy::plugins::{sample, PluginList};
use crate::state::Task;
use crate::test_support::{lock, TestCtx, TEAM};
use crate::Vocdoni;
use claudio_core::addon::{AddonEvent, Ui};

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
            crate::plugins::state::ManifestState::Fetching,
        ),
    );
    let tasks = state.on_manifest("kb", &other, Ok(manifest.clone()));
    assert!(
        !tasks.iter().any(|t| matches!(t, Task::Offer(_))),
        "{tasks:?}"
    );
    // A fresh state offers on the manifest's arrival, once.
    let mut fresh = crate::state::State::default();
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
    let mut saved = crate::state::State::default();
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

// ── Segments (the status bar's proxy and credential parts) ──────────────────

mod segment_tests {
    use crate::proxy::api::{CredentialInfo, CredentialUtilization, SessionCredential};
    use crate::state::stats::Focused;
    use crate::state::State;
    use crate::ui::segments;
    use claudio_core::addon::{LaunchChoice, Segment, SessionSnap, Tone};
    use claudio_core::proto::{SessionId, SessionKind};

    fn snap(profile: Option<&str>, conversation: Option<&str>) -> SessionSnap {
        SessionSnap {
            sid: SessionId::from_u128(1),
            host: "local".into(),
            kind: SessionKind::Claude,
            cwd: "/w".into(),
            label: "s".into(),
            conversation: conversation.map(str::to_owned),
            launch: profile.map(|p| LaunchChoice {
                addon: "vocdoni".into(),
                option: p.into(),
                flags: Vec::new(),
            }),
            focused: true,
        }
    }

    fn cred(five_hour: Option<f64>, switched_at: Option<&str>) -> SessionCredential {
        SessionCredential {
            credential: CredentialInfo {
                label: "work-max".into(),
                plan: "max".into(),
                ..Default::default()
            },
            switched_at: switched_at.map(str::to_owned),
            utilization: five_hour.map(|p| CredentialUtilization {
                five_hour_pct: Some(p),
                seven_day_pct: Some(91.0),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// A state that has `c` for session 1's conversation `c1`, at `now`.
    fn with(c: SessionCredential, now: u64) -> State {
        let mut s = State {
            now,
            ..State::default()
        };
        let focused = Focused {
            sid: SessionId::from_u128(1),
            profile: "work".into(),
            conversation: "c1".into(),
        };
        s.creds_due(vec![focused]);
        s.on_cred(SessionId::from_u128(1), "c1", Ok(Some(c)));
        s
    }

    fn texts(segs: &[Segment]) -> Vec<(&str, u8)> {
        segs.iter().map(|s| (s.text.as_str(), s.priority)).collect()
    }

    #[test]
    fn credential_shows_label_and_plan_and_the_windows_go_to_line_two() {
        let s = with(cred(Some(37.5), None), 0);
        let segs = segments(&s, &snap(Some("work"), Some("c1")));
        assert_eq!(
            texts(&segs),
            [
                ("⇄ proxy:work", 10),
                ("work-max (max)", 20),
                ("5h 37%", 110),
                ("7d 91%", 150)
            ]
        );
        assert_eq!(segs[0].tone, Tone::Accent);
        assert_eq!((segs[2].tone, segs[3].tone), (Tone::Ok, Tone::Hot));
        // A cached credential of a previous conversation is not shown;
        // a direct session gets nothing.
        let segs = segments(&s, &snap(Some("work"), Some("c2")));
        assert_eq!(texts(&segs), [("⇄ proxy:work", 10)]);
        assert!(segments(&s, &snap(None, Some("c1"))).is_empty());
    }

    #[test]
    fn credential_without_utilization_or_plan_is_just_the_label() {
        let mut c = cred(None, None);
        c.credential.plan.clear();
        let line = |c: &SessionCredential| {
            let segs = segments(&with(c.clone(), 0), &snap(Some("work"), Some("c1")));
            segs.into_iter().skip(1).map(|s| s.text).collect::<Vec<_>>()
        };
        assert_eq!(line(&c), ["work-max"]);
        // The id stands in for a missing label; nothing to name shows nothing.
        c.credential.label.clear();
        c.credential.id = "cred_ab12".into();
        assert_eq!(line(&c), ["cred_ab12"]);
        c.credential.id.clear();
        assert!(line(&c).is_empty());
        // A long name is cut.
        c.credential.label = "a-very-long-credential-name-indeed".into();
        assert_eq!(line(&c), ["a-very-long-credential-…"]);
    }

    #[test]
    fn switch_notice_lasts_ten_minutes() {
        let at = "2026-10-09T10:07:02Z";
        let t0 = claudio_core::text::parse_rfc3339(at).unwrap();
        let line = |now| {
            let segs = segments(
                &with(cred(None, Some(at)), now),
                &snap(Some("w"), Some("c1")),
            );
            segs.into_iter().skip(1).map(|s| s.text).collect::<Vec<_>>()
        };
        assert_eq!(line(t0 + 180), ["work-max (max)", "⇆ switched 3m ago"]);
        assert_eq!(line(t0 + 599), ["work-max (max)", "⇆ switched 9m ago"]);
        assert_eq!(line(t0 + 600), ["work-max (max)"]);
        let segs = segments(
            &with(cred(None, Some(at)), t0 + 5),
            &snap(Some("w"), Some("c1")),
        );
        assert_eq!((segs[2].tone, segs[2].priority), (Tone::Warn, 21));
    }
}
