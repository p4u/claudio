//! The browser's Session and Host objects (docs/web-protocol.md §4), built
//! field by field: never a serialization of a struct that could carry a
//! secret, and absent values are `null`, never skipped.

use serde_json::{json, Value};

use super::hub::{HostState, SessionView};
use crate::proto::{SessionKind, SessionState};

#[cfg_attr(not(test), allow(dead_code))] // the documented list, checked by the tests
pub(crate) const SESSION_FIELDS: &[&str] = &[
    "id",
    "host",
    "kind",
    "cwd",
    "name",
    "title",
    "label",
    "state",
    "state_since",
    "attention",
    "claude_session_id",
    "model",
    "context_tokens",
    "output_tokens",
    "turns",
    "branch",
    "git",
    "created_at",
    "pid",
    "dormant",
    "launch",
    "segments",
    "ephemeral",
    "prompt_pending",
];

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const HOST_FIELDS: &[&str] = &[
    "name",
    "connected",
    "connecting",
    "error",
    "outdated",
    "hostname",
    "os",
    "arch",
    "claude",
    "features",
    "version",
];

fn kind_name(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Claude => "claude",
        SessionKind::Shell => "shell",
    }
}

fn state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Working => "working",
        SessionState::NeedsApproval => "needs_approval",
        SessionState::NeedsInput => "needs_input",
        SessionState::Idle => "idle",
        SessionState::Error => "error",
        SessionState::Exited => "exited",
        SessionState::Unknown => "unknown",
    }
}

pub(crate) fn session(view: &SessionView) -> Value {
    let info = &view.info;
    let title = match info.kind {
        SessionKind::Claude => info.title.as_deref(),
        SessionKind::Shell => None,
    };
    let waiting = matches!(
        info.state,
        SessionState::NeedsApproval | SessionState::NeedsInput
    );
    let attention = info
        .attention
        .as_ref()
        .filter(|_| waiting)
        .map(|a| {
            json!({
                "seq": a.seq,
                "kind": a.kind,
                "message": a.message,
                "tool": a.tool,
                "detail": a.detail,
            })
        })
        .unwrap_or(Value::Null);
    let git = view
        .git
        .as_ref()
        .map(|g| {
            json!({
                "head": g.head,
                "detached": g.detached,
                "upstream": g.upstream,
                "ahead": g.ahead,
                "behind": g.behind,
                "staged": g.staged,
                "unstaged": g.unstaged,
                "untracked": g.untracked,
                "conflicts": g.conflicts,
            })
        })
        .unwrap_or(Value::Null);
    json!({
        "id": info.id,
        "host": view.host,
        "kind": kind_name(info.kind),
        "cwd": info.cwd,
        "name": info.name,
        "title": title,
        "label": label(info.kind, info.name.as_deref(), info.title.as_deref(), &info.cwd),
        "state": state_name(info.state),
        "state_since": view.state_since,
        "attention": attention,
        "claude_session_id": info.claude_session_id,
        "model": info.model,
        "context_tokens": info.context_tokens,
        "output_tokens": view.output_tokens,
        "turns": view.turns,
        "branch": info.branch,
        "git": git,
        "created_at": info.created_at,
        "pid": info.pid,
        "dormant": info.state == SessionState::Exited && info.pid.is_none(),
        "launch": super::addons::launch_dto(&view.addon),
        "segments": super::addons::segments_dto(&view.addon),
        "ephemeral": false,
        "prompt_pending": view.prompt.is_some(),
    })
}

pub(crate) fn host(state: &HostState) -> Value {
    let welcome = state.welcome.as_ref();
    let claude = welcome
        .and_then(|w| w.host.claude.as_ref())
        .map(|c| json!({"path": c.path, "version": c.version}))
        .unwrap_or(Value::Null);
    json!({
        "name": state.name,
        "connected": state.client.is_some(),
        "connecting": state.connecting,
        "error": state.error,
        "outdated": state.outdated,
        "hostname": welcome.map(|w| w.host.hostname.as_str()),
        "os": welcome.map(|w| w.host.os.as_str()),
        "arch": welcome.map(|w| w.host.arch.as_str()),
        "claude": claude,
        "features": state.features,
        "version": welcome.map(|w| w.claudio_version.as_str()),
    })
}

/// The TUI's label rule (tui::sessions::SessionView::label), via
/// `tui::sessions::sanitize_label(raw, 200)`.
pub(crate) fn label(
    kind: SessionKind,
    name: Option<&str>,
    title: Option<&str>,
    cwd: &str,
) -> String {
    let title = match kind {
        SessionKind::Claude => title,
        SessionKind::Shell => None,
    };
    let raw = name
        .map(str::to_owned)
        .or_else(|| {
            title
                .filter(|t| *t != "Claude Code" && !t.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| match kind {
            SessionKind::Claude => cwd
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or("/")
                .to_owned(),
            SessionKind::Shell => "term".to_owned(),
        });
    crate::app::label::sanitize_label(&raw, 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Attention, ClaudeInfo, HostInfo, SessionInfo, Welcome};
    use std::collections::BTreeSet;

    fn info(state: SessionState) -> SessionInfo {
        SessionInfo {
            id: uuid::Uuid::new_v4(),
            cwd: "/home/u/proj".into(),
            name: None,
            state,
            claude_session_id: None,
            title: None,
            pid: Some(7),
            created_at: 100,
            branch: None,
            model: None,
            context_tokens: None,
            kind: SessionKind::Claude,
            ephemeral: false,
            attention: None,
        }
    }

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn session_dto_has_exactly_the_documented_fields() {
        let mut view = SessionView::new("local", info(SessionState::NeedsApproval), 5);
        let want: BTreeSet<String> = SESSION_FIELDS.iter().map(|s| s.to_string()).collect();
        assert_eq!(keys(&session(&view)), want);
        view.info.attention = Some(Attention {
            seq: 1,
            kind: "permission_prompt".into(),
            message: "m".into(),
            tool: Some("Bash".into()),
            detail: None,
        });
        view.meta = Some(crate::app::state::SavedRoute {
            launch: None,
            legacy: crate::addon::LegacyChoice {
                proxy: Some("work".into()),
                first_party: true,
            },
        });
        let v = session(&view);
        assert_eq!(keys(&v), want);
        assert_eq!(v["attention"]["kind"], "permission_prompt");
        assert_eq!(v["attention"]["detail"], Value::Null);
        assert_eq!(v["state"], "needs_approval");
        for v1 in ["proxy", "proxy_known", "first_party"] {
            assert!(v.get(v1).is_none(), "{v1} is version 1's");
        }
        assert_eq!(v["git"], Value::Null);
    }

    #[test]
    fn host_dto_has_exactly_the_documented_fields() {
        let want: BTreeSet<String> = HOST_FIELDS.iter().map(|s| s.to_string()).collect();
        let mut h = HostState::new("z6");
        assert_eq!(keys(&host(&h)), want);
        assert_eq!(host(&h)["claude"], Value::Null);
        h.welcome = Some(Welcome {
            claudio_version: "0.8.0".into(),
            proto: 1,
            host: HostInfo {
                hostname: "z6.example".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                home: "/home/u".into(),
                claude: Some(ClaudeInfo {
                    path: "/usr/bin/claude".into(),
                    version: "claude 0.0.0-fake".into(),
                }),
            },
            build: None,
            build_time: None,
            features: vec!["attention".into()],
        });
        let v = host(&h);
        assert_eq!(keys(&v), want);
        assert_eq!(v["claude"]["version"], "claude 0.0.0-fake");
        assert_eq!(v["hostname"], "z6.example");
        assert!(v.get("home").is_none(), "no home directory");
    }

    #[test]
    fn label_follows_the_tui_rule() {
        use SessionKind::{Claude, Shell};
        assert_eq!(label(Claude, Some("mine"), Some("t"), "/a/b"), "mine");
        assert_eq!(
            label(Claude, None, Some("Fix the bug"), "/a/b"),
            "Fix the bug"
        );
        assert_eq!(label(Claude, None, Some("Claude Code"), "/a/b"), "b");
        assert_eq!(label(Claude, None, Some(""), "/a/b/"), "b");
        assert_eq!(label(Claude, None, None, "/"), "/");
        assert_eq!(label(Shell, None, Some("vim"), "/a/b"), "term");
        assert_eq!(label(Shell, Some("logs"), None, "/a"), "logs");
        assert_eq!(label(Claude, Some("a\u{1b}[2Jb"), None, "/"), "a[2Jb");
        assert_eq!(
            label(Claude, Some(&"x".repeat(300)), None, "/")
                .chars()
                .count(),
            200
        );
    }

    #[test]
    fn dormant_is_exited_without_a_pid() {
        let mut view = SessionView::new("local", info(SessionState::Exited), 5);
        assert_eq!(session(&view)["dormant"], false, "exited, still has a pid");
        view.info.pid = None;
        assert_eq!(session(&view)["dormant"], true);
        view.info.state = SessionState::Starting;
        assert_eq!(session(&view)["dormant"], false);
    }
}
