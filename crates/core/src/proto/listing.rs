//! The wizard's directory listing (`ListDir` → `DirEntries`): one entry, and
//! the reply cut to fit a frame. Re-exported from [`crate::proto`].

use serde::{Deserialize, Serialize};

use super::{Msg, MAX_FRAME};

/// Room left in a frame for the envelope around a `DirEntries` reply.
const ENVELOPE_SLACK: usize = 256;

/// One entry of a [`Msg::DirEntries`] listing (the wizard's directory
/// browser), with the daemon's git enrichment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    /// The git branch name when the directory is a git repo, `Some("")` for a
    /// detached/unknown HEAD, `None` when not a git repo. Filled for folders
    /// only; older daemons leave this as `None`.
    #[serde(default)]
    pub git: Option<String>,
    /// Always `None` today: no daemon fills it. Kept so the reply's shape
    /// stays the same for older clients.
    #[serde(default)]
    pub claude_at: Option<u64>,
    /// Whether the directory entry is a symlink.
    #[serde(default)]
    pub symlink: bool,
    /// Whether the entry name starts with `.`.
    #[serde(default)]
    pub hidden: bool,
}

impl DirEntry {
    /// Construct a minimal entry with no enrichment. Used in tests and by
    /// callers that do their own field assignment.
    pub fn simple(name: impl Into<String>, dir: bool) -> Self {
        DirEntry {
            name: name.into(),
            dir,
            git: None,
            claude_at: None,
            symlink: false,
            hidden: false,
        }
    }
}

/// The `DirEntries` reply for `path`, without as many of the last entries
/// (files first: `entries` is sorted folders first) as it takes for its JSON
/// to fit [`MAX_FRAME`] less the envelope's room (then `truncated`). Each
/// entry is measured as it encodes, escapes included.
pub fn dir_entries_reply(path: String, mut entries: Vec<DirEntry>, truncated: bool) -> Msg {
    let budget = MAX_FRAME - ENVELOPE_SLACK;
    let mut used = json_len(&Msg::DirEntries {
        path: path.clone(),
        entries: Vec::new(),
        truncated: true,
    });
    // One comma per entry after the first; counting one for each is safe.
    let kept = entries
        .iter()
        .take_while(|e| {
            used += json_len(e) + 1;
            used <= budget
        })
        .count();
    let cut = kept < entries.len();
    entries.truncate(kept);
    Msg::DirEntries {
        path,
        entries,
        truncated: truncated || cut,
    }
}

fn json_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX / 2, |json| json.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Envelope, Frame};

    fn frame_len(msg: Msg) -> usize {
        Frame::Control(Envelope::request(u64::MAX, msg))
            .encode()
            .len()
            - 4
    }

    #[test]
    fn a_reply_of_long_names_is_cut_to_fit_a_frame() {
        let entries: Vec<DirEntry> = (0..5000)
            .map(|i| DirEntry::simple(format!("{i:05}{}", "n".repeat(195)), true))
            .collect();
        let msg = dir_entries_reply("/srv/big".into(), entries, false);
        let Msg::DirEntries {
            entries, truncated, ..
        } = &msg
        else {
            panic!("expected DirEntries");
        };
        assert!(*truncated);
        assert!(
            entries.len() > 3000 && entries.len() < 5000,
            "{}",
            entries.len()
        );
        assert_eq!(entries[0].name.len(), 200);
        assert!(frame_len(msg) <= MAX_FRAME);
    }

    #[test]
    fn escapes_count_and_files_go_first() {
        // A control character encodes as six bytes (`\u0001`): 40 of them
        // per name make a reply the old `name + 24` estimate passed.
        let mut entries: Vec<DirEntry> = (0..20)
            .map(|i| DirEntry::simple(format!("dir{i}"), true))
            .collect();
        entries.extend(
            (0..5000).map(|i| DirEntry::simple(format!("{i}{}", "\u{1}".repeat(40)), false)),
        );
        let msg = dir_entries_reply("/x".into(), entries, false);
        let Msg::DirEntries {
            entries, truncated, ..
        } = &msg
        else {
            panic!("expected DirEntries");
        };
        assert!(*truncated);
        assert!(entries[..20].iter().all(|e| e.dir), "every folder is kept");
        assert!(frame_len(msg) <= MAX_FRAME);
    }

    #[test]
    fn a_small_listing_is_kept_whole() {
        let entries = vec![DirEntry::simple("a", true), DirEntry::simple("b", false)];
        let msg = dir_entries_reply("/x".into(), entries.clone(), false);
        assert_eq!(
            msg,
            Msg::DirEntries {
                path: "/x".into(),
                entries,
                truncated: false
            }
        );
        let Msg::DirEntries { truncated, .. } = dir_entries_reply("/x".into(), vec![], true) else {
            panic!("expected DirEntries");
        };
        assert!(truncated, "the daemon's own cut is kept");
    }
}
