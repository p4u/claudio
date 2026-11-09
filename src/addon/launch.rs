//! Launches: an addon's flags, options, routing and share of a session's
//! spawn (docs/addons.md, "Launches").

use std::fmt;
use std::sync::Arc;

use super::{AddonCtx, BoxFuture};
use crate::proto::{LensSpec, SessionId};

/// A launch flag an addon declares (`--proxy NAME`, `--first-party`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagDecl {
    /// `--[a-z][a-z0-9-]*`.
    pub name: &'static str,
    /// The value's placeholder (`NAME`), or `None` for a switch.
    pub value: Option<&'static str>,
    /// One line for `--help`.
    pub help: &'static str,
    /// Shown as a wizard checkbox when set (D5); only switches.
    pub label: Option<&'static str>,
}

/// A declared flag as given on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagArg {
    /// The flag, `--name`.
    pub name: String,
    /// Its value (`None` for a switch).
    pub value: Option<String>,
}

/// What a session was launched with, persisted (state.json, web-state.json).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchChoice {
    /// The addon whose option it is.
    pub addon: String,
    /// The addon's option id.
    pub option: String,
    /// The addon's switches in effect (`--first-party`).
    #[serde(default)]
    pub flags: Vec<String>,
}

/// The route a member picked.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "pick", rename_all = "snake_case")]
pub enum Pick {
    /// Whatever the addons' defaults say.
    #[default]
    Default,
    /// No addon routes this session.
    Direct,
    /// One addon's option.
    Option { addon: String, option: String },
}

/// What the member asked for: from flags, the wizard or a session's persisted choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchSelect {
    /// The route.
    #[serde(flatten)]
    pub pick: Pick,
    /// The switches in effect.
    #[serde(default)]
    pub flags: Vec<String>,
}

impl LaunchSelect {
    /// A persisted choice as a select (`None` = direct).
    pub fn of(choice: Option<&LaunchChoice>) -> LaunchSelect {
        match choice {
            None => LaunchSelect {
                pick: Pick::Direct,
                flags: Vec::new(),
            },
            Some(c) => LaunchSelect {
                pick: Pick::Option {
                    addon: c.addon.clone(),
                    option: c.option.clone(),
                },
                flags: c.flags.clone(),
            },
        }
    }
}

/// One row of the wizard's launch choices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LaunchOption {
    /// The option's id, unique within the addon.
    pub id: String,
    /// What the member sees.
    pub label: String,
    /// Options of one group share a select.
    pub group: String,
    /// Whether this is the addon's configured default.
    pub default: bool,
    /// One more line (a URL, an account).
    pub detail: Option<String>,
}

/// An addon's answer to a launch's selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing {
    /// This addon's option is in effect (chosen or its default); the
    /// launch fails if its `prepare` fails.
    Required(LaunchChoice),
    /// It contributes if it can.
    Optional,
    /// It sits this launch out.
    Off,
}

/// What kind of launch this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOp {
    /// A new session.
    New,
    /// A new session resuming a conversation.
    Resume,
    /// A session restarted in place (Alt+e, reset).
    Respawn,
    /// A journaled session restarted by a fresh daemon.
    Recover,
}

/// The front-end a launch comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontend {
    /// The console manager.
    Tui,
    /// `claudio --plain`.
    Plain,
    /// `claudio web`.
    Web,
}

/// What `Launch::prepare` is asked to prepare.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchRequest {
    /// New, resume, respawn or recover.
    pub op: LaunchOp,
    /// The session's id.
    pub sid: SessionId,
    /// The host it runs on.
    pub host: String,
    /// Its working directory.
    pub cwd: String,
    /// The launch's effective choice (the Required addon's), `None` = direct.
    pub choice: Option<LaunchChoice>,
    /// This addon's routing.
    pub routing: Routing,
    /// The front-end launching it.
    pub frontend: Frontend,
    /// This addon's checklist ids skipped for this launch.
    pub deferred: Vec<String>,
}

/// One claude argument of a launch.
#[derive(Clone, PartialEq, Eq)]
pub enum LaunchArg {
    /// Passed as is.
    Plain(String),
    /// `flag value`, dropped by the daemon when its claude lacks `flag` (D6).
    Optional { flag: String, value: String },
}

/// An addon's share of a launch. `Debug` prints env keys only (C1).
#[derive(Clone, Default, PartialEq)]
pub struct LaunchPart {
    /// Env for the session; secrets live here and only here.
    pub env: Vec<(String, String)>,
    /// Names the daemon removes from the session's inherited env.
    pub env_remove: Vec<String>,
    /// Claude arguments, appended in registry order.
    pub args: Vec<LaunchArg>,
    /// Lenses for the daemon to extract.
    pub lenses: Vec<LensSpec>,
    /// Instructions for claude, joined with the other addons'.
    pub instructions: String,
    /// Shown once the session starts.
    pub notices: Vec<String>,
}

impl fmt::Debug for LaunchPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("LaunchPart")
            .field("env", &env)
            .field("env_remove", &self.env_remove)
            .field("args", &self.args)
            .field("lenses", &self.lenses)
            .field(
                "instructions",
                &format_args!("<{} bytes>", self.instructions.len()),
            )
            .field("notices", &self.notices)
            .finish()
    }
}

impl fmt::Debug for LaunchArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchArg::Plain(s) => f.debug_tuple("Plain").field(s).finish(),
            LaunchArg::Optional { flag, value } => f
                .debug_struct("Optional")
                .field("flag", flag)
                .field("value", &format_args!("<{} bytes>", value.len()))
                .finish(),
        }
    }
}

/// Why `Launch::prepare` gave no part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// Show checklist `checklist`, then prepare again (D8).
    NeedsReview { checklist: String, revision: String },
    /// The addon cannot contribute; fails the launch when it is Required.
    Failed(String),
}

/// The pre-P3 persisted routing (state.json/web-state.json `proxy`, `first_party`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyChoice {
    /// The proxy profile, if any.
    pub proxy: Option<String>,
    /// Whether the session was first-party.
    pub first_party: bool,
}

/// An addon's part in launches (docs/addons.md, "Launches").
pub trait Launch: Send + Sync {
    /// The flags it parses on `claudio …` and `claudio --plain …`.
    fn flags(&self) -> &'static [FlagDecl] {
        &[]
    }
    /// This addon's flags as given on the command line → a select (`Err`: a usage message).
    fn select(&self, given: &[FlagArg]) -> Result<LaunchSelect, String> {
        let _ = given;
        Ok(LaunchSelect::default())
    }
    /// The wizard's rows for this addon. No I/O.
    fn options(&self, ctx: &dyn AddonCtx) -> Vec<LaunchOption> {
        let _ = ctx;
        Vec::new()
    }
    /// `Err`: an unknown or unusable choice of this addon (fails the launch). No I/O.
    fn resolve(&self, select: &LaunchSelect, ctx: &dyn AddonCtx) -> Result<Routing, String>;
    /// This addon's share of the launch, within the launch's 25 s budget.
    fn prepare<'a>(
        &'a self,
        req: LaunchRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<LaunchPart, LaunchError>>;
    /// A pre-P3 persisted routing as this addon's choice, if it is one.
    fn adopt_legacy(&self, legacy: &LegacyChoice) -> Option<LaunchChoice> {
        let _ = legacy;
        None
    }
    /// The pre-P3 fields to keep writing for `choice`, if it is this addon's.
    fn legacy(&self, choice: &LaunchChoice) -> Option<LegacyChoice> {
        let _ = choice;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_part_debug_shows_env_keys_never_values() {
        let part = LaunchPart {
            env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "sekret-token-value".into())],
            env_remove: vec!["ANTHROPIC_API_KEY".into()],
            args: vec![
                LaunchArg::Plain("--verbose".into()),
                LaunchArg::Optional {
                    flag: "--mcp-config".into(),
                    value: "{\"secret\":\"sekret-arg-value\"}".into(),
                },
            ],
            lenses: Vec::new(),
            instructions: "sekret-instructions".into(),
            notices: vec!["hello".into()],
        };
        let shown = format!("{part:?}");
        assert!(shown.contains("ANTHROPIC_AUTH_TOKEN"), "{shown}");
        assert!(shown.contains("ANTHROPIC_API_KEY"), "{shown}");
        assert!(shown.contains("--verbose") && shown.contains("--mcp-config"));
        assert!(shown.contains("<19 bytes>"), "{shown}");
        assert!(!shown.contains("sekret"), "{shown}");
        let pretty = format!("{part:#?}");
        assert!(!pretty.contains("sekret"), "{pretty}");
    }

    #[test]
    fn launch_select_json_round_trips() {
        let default: LaunchSelect = serde_json::from_str(r#"{"pick":"default"}"#).unwrap();
        assert_eq!(default, LaunchSelect::default());
        assert_eq!(
            serde_json::to_value(&default).unwrap(),
            serde_json::json!({"pick": "default", "flags": []})
        );
        let text = r#"{"pick":"option","addon":"a","option":"b","flags":["x"]}"#;
        let option: LaunchSelect = serde_json::from_str(text).unwrap();
        assert_eq!(
            option,
            LaunchSelect {
                pick: Pick::Option {
                    addon: "a".into(),
                    option: "b".into()
                },
                flags: vec!["x".into()],
            }
        );
        let back: LaunchSelect =
            serde_json::from_value(serde_json::to_value(&option).unwrap()).unwrap();
        assert_eq!(back, option);
        let direct: LaunchSelect = serde_json::from_str(r#"{"pick":"direct"}"#).unwrap();
        assert_eq!(direct, LaunchSelect::of(None));
        let choice = LaunchChoice {
            addon: "a".into(),
            option: "b".into(),
            flags: vec!["x".into()],
        };
        assert_eq!(LaunchSelect::of(Some(&choice)), option);
        // A choice persisted without flags still loads.
        let old: LaunchChoice = serde_json::from_str(r#"{"addon":"a","option":"b"}"#).unwrap();
        assert!(old.flags.is_empty());
    }
}
