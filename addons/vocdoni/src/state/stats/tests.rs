//! The credential lookups' and the stats cache's tests (the TUI's
//! `refresh_session_cred` and `on_session_credential` rules, as tasks).

use super::*;
use crate::proxy::api::CredentialInfo;

fn focused(sid: u128, conversation: &str) -> Focused {
    Focused {
        sid: SessionId::from_u128(sid),
        profile: "work".into(),
        conversation: conversation.into(),
    }
}

fn named(label: &str) -> SessionCredential {
    SessionCredential {
        credential: CredentialInfo {
            label: label.into(),
            ..CredentialInfo::default()
        },
        ..SessionCredential::default()
    }
}

fn asks(tasks: &[Task]) -> Vec<String> {
    tasks
        .iter()
        .filter_map(|t| match t {
            Task::FetchCred { conversation, .. } => Some(conversation.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_focused_sessions_credential_is_polled_every_thirty_seconds() {
    let mut s = State {
        now: 1_000,
        ..State::default()
    };
    let sid = SessionId::from_u128(1);
    assert_eq!(asks(&s.creds_due(vec![focused(1, "c1")])), ["c1"]);
    s.now += 1;
    assert!(s.creds_due(vec![focused(1, "c1")]).is_empty(), "asked");
    assert_eq!(
        s.on_cred(sid, "c1", Ok(Some(named("work-max")))),
        [Task::Changed]
    );
    assert_eq!(
        s.cred(sid, Some("c1")).and_then(|c| c.name()),
        Some("work-max")
    );
    s.now = 1_029;
    assert!(s.creds_due(vec![focused(1, "c1")]).is_empty());
    s.now = 1_030;
    assert_eq!(asks(&s.creds_due(vec![focused(1, "c1")])), ["c1"]);
    // Kept while asked again; an error keeps it, a 404 clears it.
    assert!(s.cred(sid, Some("c1")).is_some());
    assert!(s.on_cred(sid, "c1", Err("rate limited".into())).is_empty());
    assert!(s.cred(sid, Some("c1")).is_some());
    assert_eq!(s.on_cred(sid, "c1", Ok(None)), [Task::Changed]);
    assert!(s.cred(sid, Some("c1")).is_none());
    // Unfocused: not asked.
    s.now = 2_000;
    assert!(s.creds_due(Vec::new()).is_empty());
}

#[test]
fn a_new_conversation_is_asked_after_the_gap_and_old_answers_are_dropped() {
    let mut s = State {
        now: 1_000,
        ..State::default()
    };
    let sid = SessionId::from_u128(1);
    s.creds_due(vec![focused(1, "c1")]);
    s.on_cred(sid, "c1", Ok(Some(named("a"))));
    // `/clear`: a new conversation, asked once the 5 s gap passed.
    s.now = 1_003;
    assert!(s.creds_due(vec![focused(1, "c2")]).is_empty());
    assert!(s.cred(sid, Some("c2")).is_none(), "the old one is not c2's");
    s.now = 1_005;
    assert_eq!(asks(&s.creds_due(vec![focused(1, "c2")])), ["c2"]);
    // A late answer for c1 is dropped.
    assert!(s.on_cred(sid, "c1", Ok(Some(named("a")))).is_empty());
    assert!(s.cred(sid, Some("c2")).is_none());
    s.on_cred(sid, "c2", Ok(Some(named("b"))));
    assert_eq!(s.cred(sid, Some("c2")).and_then(|c| c.name()), Some("b"));
    // After a switch it is asked on the next second; gone with its session.
    s.cred_stale(sid);
    assert_eq!(asks(&s.creds_due(vec![focused(1, "c2")])), ["c2"]);
    s.forget_session(sid);
    assert!(s.cred(sid, Some("c2")).is_none());
}

#[test]
fn stats_merge_by_window_and_keep_stale_numbers_on_error() {
    let mut s = State {
        now: 50,
        ..State::default()
    };
    let ok = ProxyFetch {
        stats: vec![(Window::D7, Ok(Default::default()))],
        ..ProxyFetch::default()
    };
    s.on_stats("work", ok);
    let failed = ProxyFetch {
        stats: vec![(Window::H24, Err("rate limited".into()))],
        ..ProxyFetch::default()
    };
    s.now = 60;
    s.on_stats("work", failed);
    let status = &s.stats["work"];
    assert_eq!(status.cached_windows(), [Window::D7]);
    assert_eq!(status.error.as_deref(), Some("rate limited"));
    assert_eq!(status.fetched_at, Some(60));
}
