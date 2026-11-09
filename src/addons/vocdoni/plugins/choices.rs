//! What a member saves for a plugin from its setup screen (TUI and web).

use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::config::PluginChoices;

/// The choices to save for `m` from a screen whose off ids are `off`:
/// `disabled` keeps the earlier disabled ids `m` no longer has (so one that
/// comes back stays off), and `seen` is every id of `m` plus those.
pub fn choices_for(m: &Manifest, off: &[String], earlier: Option<&PluginChoices>) -> PluginChoices {
    let ids = m.ids();
    let gone: Vec<String> = earlier
        .map(|c| {
            c.disabled
                .iter()
                .filter(|d| !ids.contains(d))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut disabled = off.to_vec();
    disabled.extend(gone.iter().cloned());
    let mut seen = ids;
    seen.extend(gone);
    PluginChoices {
        disabled,
        reviewed: m.version.clone(),
        seen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = r#"
format = 1
name = "team"
version = "1.0.0"
instructions = "Rules."
[[marketplace]]
name = "m"
source = "github:o/r"
[[plugin]]
name = "a"
marketplace = "m"
[[mcp]]
name = "kb"
type = "sse"
url = "https://kb.example.org"
[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review."
"#;

    fn team() -> Manifest {
        Manifest::parse(TEAM).unwrap()
    }

    #[test]
    fn saved_choices_keep_the_disabled_ids_a_manifest_dropped() {
        let earlier = PluginChoices {
            disabled: vec!["mcp:gone".into(), "mcp:kb".into()],
            reviewed: "0.9".into(),
            seen: vec![],
        };
        let c = choices_for(&team(), &["agent:reviewer".into()], Some(&earlier));
        assert_eq!(c.disabled, ["agent:reviewer", "mcp:gone"]);
        assert_eq!(c.reviewed, "1.0.0");
        assert_eq!(
            c.seen,
            [
                "plugin:a@m",
                "mcp:kb",
                "agent:reviewer",
                "instructions",
                "mcp:gone"
            ]
        );
    }
}
