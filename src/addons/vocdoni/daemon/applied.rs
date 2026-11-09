//! The `vocdoni/apply` daemon op on the wire (docs/addons.md, "The Vocdoni
//! addon"): its request body, its reply body, and the client's halves.
//! Serde-identical to the `Manifest`/`Applied` messages it replaces.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::proto::Msg;

/// The Vocdoni addon's daemon op that applies a plugin manifest.
pub const OP_APPLY: &str = "apply";

/// The body of `vocdoni/apply`: apply proxy plugin `name`'s manifest on
/// this host, the TOML at `url` (https) whose SHA-256 is `sha256`, cached
/// once downloaded. `secrets` are the names (never the values) of the
/// plugin's proxy env, which the sessions get; `disabled` the item ids the
/// member turned off.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub name: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// What a host made of a proxy plugin's manifest: what every new session
/// there gets from it, and each item's outcome by item id
/// (`plugin:<name>@<marketplace>`, `mcp:<name>`, `agent:<name>`…). A
/// disabled id is in no list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub name: String,
    pub version: String,
    /// `{"mcpServers": {...}}` for `--mcp-config` (≤ 64 KiB): `${NAME}`
    /// placeholders, never a secret.
    #[serde(default)]
    pub mcp_config: Option<String>,
    /// The `--agents` JSON (≤ 32 KiB).
    #[serde(default)]
    pub agents: Option<String>,
    /// For the session context: sanitized, ≤ 8 KiB, empty when none.
    #[serde(default)]
    pub instructions: String,
    /// The manifest's `[env]`.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Plugin ids this daemon installed (under this manifest's hash).
    #[serde(default)]
    pub installed: Vec<String>,
    /// Plugin ids Claude Code had installed already.
    #[serde(default)]
    pub present: Vec<String>,
    /// `(id, reason)`: `installing`, `yours`, `missing secret X`, a
    /// failure…
    #[serde(default)]
    pub skipped: Vec<(String, String)>,
    /// `--mcp-config` and/or `--agents` when this host's claude lacks them.
    #[serde(default)]
    pub dropped: Vec<String>,
}

/// The reason in [`Applied::skipped`] of a failed plugin install the daemon
/// tries again in `secs` seconds.
pub fn retrying(why: &str, secs: u64) -> String {
    format!("{why} (retrying in {secs} s)")
}

/// The seconds of a [`retrying`] reason.
pub fn retry_secs(reason: &str) -> Option<u64> {
    let (_, secs) = reason.strip_suffix(" s)")?.rsplit_once(" (retrying in ")?;
    secs.parse().ok()
}

/// Whether a daemon whose `Welcome.features` are `features` takes
/// `vocdoni/apply` (lists `addon:vocdoni`); an older one is never asked.
pub fn serves_apply(features: &[String]) -> bool {
    features.contains(&crate::addon::feature(super::super::NAME))
}

/// The `AddonOp` that asks a host to apply `req`. Its body names secrets,
/// never their values.
pub fn apply_msg(req: &ApplyRequest) -> Msg {
    Msg::AddonOp {
        addon: super::super::NAME.to_owned(),
        op: OP_APPLY.to_owned(),
        body: serde_json::to_value(req).unwrap_or(Value::Null),
    }
}

/// The `Applied` of a host's reply to [`apply_msg`]: `Err` (a reason for
/// the member) for any other message or a body that is not one.
pub fn applied_of(reply: Msg) -> Result<Applied, String> {
    match reply {
        Msg::AddonReply { body } => {
            serde_json::from_value(body).map_err(|_| "unexpected reply".to_owned())
        }
        _ => Err("unexpected reply".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retrying_reason_carries_its_seconds() {
        let why = retrying("marketplace m: down (503)", 15);
        assert_eq!(why, "marketplace m: down (503) (retrying in 15 s)");
        assert_eq!(retry_secs(&why), Some(15));
        for other in [
            "installing",
            "network down",
            "x (retrying in s)",
            "(retrying in 5 s",
        ] {
            assert_eq!(retry_secs(other), None, "{other}");
        }
    }

    /// The request and the reply round-trip as `AddonOp`/`AddonReply`
    /// bodies; every list may be left out; an unknown request field is
    /// refused. (Replaces proto's
    /// `manifest_and_applied_roundtrip_and_are_unknown_to_older_peers`.)
    #[test]
    fn apply_request_and_applied_roundtrip_as_addon_bodies() {
        let req = ApplyRequest {
            name: "team".into(),
            url: "https://e.org/team.toml".into(),
            sha256: "a".repeat(64),
            secrets: vec!["KB_TOKEN".into()],
            disabled: vec!["mcp:jina".into()],
        };
        let msg = apply_msg(&req);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "addon_op");
        assert_eq!(json["addon"], "vocdoni");
        assert_eq!(json["name"], "apply");
        assert_eq!(json["body"]["secrets"], serde_json::json!(["KB_TOKEN"]));
        let Msg::AddonOp { body, .. } = msg else {
            panic!("not an addon op");
        };
        assert_eq!(serde_json::from_value::<ApplyRequest>(body).unwrap(), req);
        let minimal = serde_json::json!({"name": "t", "url": "https://e.org", "sha256": "x"});
        let back: ApplyRequest = serde_json::from_value(minimal).unwrap();
        assert!(back.secrets.is_empty() && back.disabled.is_empty());
        let extra = serde_json::json!({"name": "t", "url": "u", "sha256": "x", "token": "v"});
        assert!(serde_json::from_value::<ApplyRequest>(extra).is_err());

        let applied = Applied {
            name: "team".into(),
            version: "1.0.0".into(),
            mcp_config: Some(r#"{"mcpServers":{}}"#.into()),
            agents: None,
            instructions: "Rules.".into(),
            env: vec![("REGION".into(), "eu".into())],
            installed: vec!["plugin:a@m".into()],
            present: vec!["plugin:b@m".into()],
            skipped: vec![("plugin:c@m".into(), "installing".into())],
            dropped: vec!["--agents".into()],
        };
        let reply = Msg::AddonReply {
            body: serde_json::to_value(&applied).unwrap(),
        };
        assert_eq!(applied_of(reply), Ok(applied));
        let minimal = serde_json::json!({"name": "team", "version": "1"});
        let reply = Msg::AddonReply { body: minimal };
        let want = Applied {
            name: "team".into(),
            version: "1".into(),
            ..Applied::default()
        };
        assert_eq!(applied_of(reply), Ok(want));
        assert!(applied_of(Msg::Ok).is_err());
    }
}
