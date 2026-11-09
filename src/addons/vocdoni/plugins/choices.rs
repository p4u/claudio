//! What a member saves for a plugin from its setup screen (TUI and web):
//! `[plugins.<name>]` of config.toml, a section the Vocdoni addon owns.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::addons::vocdoni::plugins::manifest::Manifest;

/// `[plugins.<name>]`: what the member chose for proxy plugin `<name>`'s
/// manifest. Written when the setup screen closes with Enter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginChoices {
    /// The item ids turned off (`mcp:jina`, `plugin:x@m`…), including ids
    /// the manifest no longer has, so one that comes back stays off.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// The manifest version last reviewed.
    #[serde(default)]
    pub reviewed: String,
    /// Every id of the reviewed version (and the disabled ids it lacks): an
    /// id of a newer version not in here is new.
    #[serde(default)]
    pub seen: Vec<String>,
}

/// The choices of the `[plugins]` section `section`, by plugin name; an
/// entry that is not a table of choices is left out.
pub fn from_table(section: &Table) -> BTreeMap<String, PluginChoices> {
    section
        .iter()
        .filter_map(|(name, value)| {
            let choices = value.clone().try_into().ok()?;
            Some((name.clone(), choices))
        })
        .collect()
}

/// Set plugin `name`'s table to `choices` in the `[plugins]` section
/// `section` (the edit `AddonCtx::edit_config` and the config editor take).
pub fn set(section: &mut Table, name: &str, choices: &PluginChoices) {
    let strings = |list: &[String]| Value::Array(list.iter().cloned().map(Value::String).collect());
    let mut table = Table::new();
    table.insert("disabled".into(), strings(&choices.disabled));
    table.insert("reviewed".into(), Value::String(choices.reviewed.clone()));
    table.insert("seen".into(), strings(&choices.seen));
    section.insert(name.to_owned(), Value::Table(table));
}

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
    use crate::app::config_edit::{edit_section, edit_text};

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

    fn choices(disabled: &[&str], reviewed: &str, seen: &[&str]) -> PluginChoices {
        let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        PluginChoices {
            disabled: own(disabled),
            reviewed: reviewed.into(),
            seen: own(seen),
        }
    }

    /// `text` with `[plugins.<name>]` set to `c`, as the config editor
    /// writes it.
    fn set_in(text: &str, name: &str, c: &PluginChoices) -> String {
        let table: Table = text.parse().unwrap();
        let old = match table.get("plugins") {
            Some(Value::Table(t)) => t.clone(),
            _ => Table::new(),
        };
        let mut new = old.clone();
        set(&mut new, name, c);
        edit_text(text, "plugins", &old, &new).unwrap()
    }

    /// Moved from `config::tests` (fixtures kept). The config editor now
    /// writes the table (it was a dedicated whole-table writer); the bytes
    /// for a missing table are the same.
    #[test]
    fn plugin_choices_append_the_table_when_missing() {
        let c = choices(&["mcp:jina"], "1.0.0", &["mcp:jina", "plugin:a@m"]);
        assert_eq!(
            set_in("[ui]\nnotify = false", "team", &c),
            "[ui]\nnotify = false\n\n[plugins.team]\ndisabled = [\"mcp:jina\"]\n\
             reviewed = \"1.0.0\"\nseen = [\"mcp:jina\", \"plugin:a@m\"]\n"
        );
        let none = choices(&[], "2", &[]);
        assert_eq!(
            set_in("", "team", &none),
            "[plugins.team]\ndisabled = []\nreviewed = \"2\"\nseen = []\n"
        );
        // A version with quotes and backslashes is escaped.
        let odd = choices(&[], "1.0 \"rc\" \\", &[]);
        let table: Table = set_in("", "team", &odd).parse().unwrap();
        assert_eq!(
            from_table(table["plugins"].as_table().unwrap())["team"],
            odd
        );
    }

    /// Moved from `config::tests` (same fixtures). The contract changed
    /// with the editor (B3, minimal edits): only the changed key lines are
    /// rewritten, so a key's own layout stays where its value did not
    /// change, and everything outside `[plugins.team]` stays byte for byte.
    #[test]
    fn plugin_choices_replace_the_table_and_leave_the_rest_untouched() {
        let text = "# mine\r\n[plugins.team]\r\ndisabled = []\r\nreviewed = \"0.9\"\r\n\
                    seen = [\r\n  \"mcp:kb\",\r\n]\r\n\r\n# next\r\n[plugins.other]\r\n\
                    reviewed = \"1\"\r\n[ui]\r\nnotify = true\r\n";
        let c = choices(&["mcp:kb"], "1.0", &["mcp:kb"]);
        assert_eq!(
            set_in(text, "team", &c),
            "# mine\r\n[plugins.team]\r\ndisabled = [\"mcp:kb\"]\r\nreviewed = \"1.0\"\r\n\
             seen = [\r\n  \"mcp:kb\",\r\n]\r\n\r\n# next\r\n[plugins.other]\r\n\
             reviewed = \"1\"\r\n[ui]\r\nnotify = true\r\n"
        );
        // The last table of the file.
        let text = "[ui]\nnotify = true\n\n[plugins.team]\nreviewed = \"1\"\n";
        assert_eq!(
            set_in(text, "team", &c),
            "[ui]\nnotify = true\n\n[plugins.team]\nreviewed = \"1.0\"\n\
             disabled = [\"mcp:kb\"]\nseen = [\"mcp:kb\"]\n"
        );
    }

    /// Moved from `config::tests::save_plugin_choices_writes_only_its_table`
    /// (same fixture): saved through `config_edit::edit_section`, which
    /// keeps the file's mode; an unsupported layout is refused for the
    /// whole `[plugins]` section now, and nothing is written.
    #[test]
    fn saving_choices_writes_only_its_table() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("claudio-cfg-ext-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let text = "[proxy]\ndefault = \"work\"\n\n[extensions]\nkb = false\n";
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let save = |c: &PluginChoices| edit_section(&path, "plugins", |t| set(t, "team", c));
        let read = || -> BTreeMap<String, PluginChoices> {
            let table: Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
            table
                .get("plugins")
                .and_then(Value::as_table)
                .map(from_table)
                .unwrap_or_default()
        };
        let c = choices(&["env"], "1.0.0", &["env", "mcp:kb"]);
        save(&c).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with(text), "{saved}");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(read().get("team"), Some(&c));
        // Saved again with other choices: replaced, not appended.
        let c2 = choices(&[], "1.1.0", &["env", "mcp:kb", "agent:x"]);
        save(&c2).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved.matches("[plugins.team]").count(), 1, "{saved}");
        assert_eq!(read().get("team"), Some(&c2));
        // A layout the edit cannot handle is refused, and nothing is written.
        let dotted = "plugins.team.reviewed = \"1\"\n";
        std::fs::write(&path, dotted).unwrap();
        let err = save(&c).unwrap_err();
        assert_eq!(
            err.to_string(),
            "config.toml layout not understood; edit [plugins] by hand"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), dotted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_section_entry_that_is_not_choices_is_left_out() {
        let table: Table = "team = 3\n[ok]\nreviewed = \"1\"\n[bad]\nseen = \"x\"\n"
            .parse()
            .unwrap();
        let read = from_table(&table);
        assert_eq!(read.keys().collect::<Vec<_>>(), ["ok"]);
        assert_eq!(read["ok"].reviewed, "1");
    }
}
