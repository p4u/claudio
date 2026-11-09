//! What the web's protocol v1 adapter reads of the plugins (deleted with
//! the v1 ops, core-addons plan P3.16): a panel no front-end lists, its
//! data in tables, so the adapter needs no access to the addon itself.
//! Page `plugins`: one row per plugin of every list. Page `status`: what
//! each host answered for each plugin, and the ids it skipped. Names, ids,
//! versions and reasons only, never an env value.

use super::plugins::state::{ApplyState, ManifestState};
use super::state::State;
use crate::addon::{Block, Page, Panel};

/// The panel's id.
pub const PANEL: &str = "plugins-v1";

/// The `plugins` table's columns.
pub const PLUGINS: [&str; 8] = [
    "name",
    "version",
    "profile",
    "refused",
    "manifest",
    "manifest_error",
    "choices_saved",
    "description",
];
/// The `status` table's columns (lists joined by a space).
pub const STATUS: [&str; 7] = [
    "host",
    "name",
    "state",
    "installed",
    "present",
    "dropped",
    "error",
];
/// The skipped ids' columns.
pub const SKIPPED: [&str; 4] = ["host", "name", "id", "reason"];

fn header(cols: &[&str]) -> Vec<String> {
    cols.iter().map(|c| (*c).to_owned()).collect()
}

/// The panel, from `state`.
pub(super) fn panel(state: &State) -> Panel {
    let plugins = Block::Table {
        header: header(&PLUGINS),
        rows: plugin_rows(state),
    };
    let (status, skipped) = status_rows(state);
    let status = vec![
        Block::Table {
            header: header(&STATUS),
            rows: status,
        },
        Block::Table {
            header: header(&SKIPPED),
            rows: skipped,
        },
    ];
    Panel {
        title: PANEL.to_owned(),
        pages: vec![
            Page {
                title: "plugins".to_owned(),
                blocks: vec![plugins],
            },
            Page {
                title: "status".to_owned(),
                blocks: status,
            },
        ],
        actions: Vec::new(),
        updated: None,
    }
}

/// Every plugin of every list: its manifest's state at the list's hash.
fn plugin_rows(state: &State) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for row in state.rows() {
        let p = row.plugin;
        let manifest = state.plugins.manifests.get(&p.name);
        let (manifest, error, description) = match (&row.refused, manifest) {
            (Some(why), _) => ("failed", why.clone(), String::new()),
            (None, Some((sha, ManifestState::Ready(m)))) if *sha == p.bundle_sha256 => {
                ("ready", String::new(), m.description.clone())
            }
            (None, Some((_, ManifestState::Failed(why)))) => ("failed", why.clone(), String::new()),
            (None, _) => ("fetching", String::new(), String::new()),
        };
        let saved = state.plugins.choices.contains_key(&p.name);
        out.push(vec![
            p.name.clone(),
            p.version.clone(),
            row.profile.to_owned(),
            row.refused.clone().unwrap_or_default(),
            manifest.to_owned(),
            error,
            saved.to_string(),
            description,
        ]);
    }
    out
}

/// Each `(host, plugin)` answer, in order, and the ids each skipped.
fn status_rows(state: &State) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    let mut all: Vec<_> = state.plugins.applies.iter().collect();
    all.sort_by(|a, b| a.0.cmp(b.0));
    let (mut status, mut skipped) = (Vec::new(), Vec::new());
    for ((host, name), apply) in all {
        let (word, applied, error) = match &apply.state {
            ApplyState::Pending => ("pending", None, String::new()),
            ApplyState::Ready(a) => ("ready", Some(a.as_ref()), String::new()),
            ApplyState::Failed(why) => ("failed", None, why.clone()),
            ApplyState::TooOld => ("too_old", None, String::new()),
        };
        let joined = |pick: fn(&super::daemon::Applied) -> &Vec<String>| {
            applied.map(|a| pick(a).join(" ")).unwrap_or_default()
        };
        status.push(vec![
            host.clone(),
            name.clone(),
            word.to_owned(),
            joined(|a| &a.installed),
            joined(|a| &a.present),
            joined(|a| &a.dropped),
            error,
        ]);
        for (id, reason) in applied.map(|a| a.skipped.as_slice()).unwrap_or_default() {
            skipped.push(vec![host.clone(), name.clone(), id.clone(), reason.clone()]);
        }
    }
    (status, skipped)
}
