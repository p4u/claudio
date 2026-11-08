//! What the web may type into a session for the user: an approval key,
//! guarded by the session's state and attention, and a paste, bracketed and
//! stripped of what could escape the brackets.

use crate::proto::{Attention, SessionState};

pub(crate) const STALE: &str = "stale";
pub(crate) const NOT_PERMISSION: &str = "not a permission prompt";
pub(crate) const TOO_LARGE: &str = "too large";
/// Enter: picks the option claude highlights by default ("Yes").
pub(crate) const APPROVE_KEY: &[u8] = b"\r";
/// Esc: rejects the tool and interrupts the turn.
pub(crate) const DENY_KEY: &[u8] = b"\x1b";
/// The most text one paste takes, in bytes.
pub(crate) const PASTE_MAX: usize = 64 * 1024;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Approve,
    Deny,
}

/// The key `session.approve` may send, or why not. `attention` is the
/// server's current one for the session and `has_feature` whether its
/// daemon reports attentions at all; without, `attention_seq` must be 0
/// (real attentions start at 1, so 0 never matches one).
pub(crate) fn approve_key(
    state: SessionState,
    attention: Option<&Attention>,
    has_feature: bool,
    decision: Decision,
    attention_seq: u64,
) -> Result<&'static [u8], &'static str> {
    if state != SessionState::NeedsApproval {
        return Err(STALE);
    }
    if has_feature {
        let Some(a) = attention else {
            return Err(STALE);
        };
        if a.seq != attention_seq {
            return Err(STALE);
        }
        if !matches!(
            a.kind.as_str(),
            "permission_prompt" | "worker_permission_prompt"
        ) {
            return Err(NOT_PERMISSION);
        }
    } else if attention_seq != 0 {
        return Err(STALE);
    }
    Ok(match decision {
        Decision::Approve => APPROVE_KEY,
        Decision::Deny => DENY_KEY,
    })
}

/// The bytes of a paste of `text`: `ESC[200~`, the text, `ESC[201~`, then
/// `\r` when `submit`. Line ends become `\n`; any `ESC[201~` in the text and
/// every control but `\t` and `\n` (C0, DEL, C1) are removed, so the text
/// can neither end the paste early nor send keys.
pub(crate) fn paste_bytes(text: &str, submit: bool) -> Result<Vec<u8>, &'static str> {
    if text.len() > PASTE_MAX {
        return Err(TOO_LARGE);
    }
    let text = text
        .replace(PASTE_END, "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let clean: String = text
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect();
    let mut out = Vec::with_capacity(clean.len() + 2 * PASTE_START.len() + 1);
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(clean.as_bytes());
    out.extend_from_slice(PASTE_END.as_bytes());
    if submit {
        out.push(b'\r');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::{Idle, NeedsApproval, NeedsInput, Working};

    fn attention(seq: u64, kind: &str) -> Attention {
        Attention {
            seq,
            kind: kind.into(),
            message: String::new(),
            tool: None,
            detail: None,
        }
    }

    #[test]
    fn approve_needs_the_state_the_kind_and_the_seq() {
        let a = attention(4, "permission_prompt");
        assert_eq!(
            approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 4),
            Ok(APPROVE_KEY)
        );
        assert_eq!(
            approve_key(NeedsApproval, Some(&a), true, Decision::Deny, 4),
            Ok(DENY_KEY)
        );
        assert_eq!(
            approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 3),
            Err(STALE)
        );
        assert_eq!(
            approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 5),
            Err(STALE)
        );
        assert_eq!(
            approve_key(Working, Some(&a), true, Decision::Approve, 4),
            Err(STALE)
        );
        assert_eq!(
            approve_key(NeedsInput, Some(&a), true, Decision::Approve, 4),
            Err(STALE)
        );
        assert_eq!(
            approve_key(NeedsApproval, None, true, Decision::Approve, 0),
            Err(STALE)
        );
        let w = attention(2, "worker_permission_prompt");
        assert_eq!(
            approve_key(NeedsApproval, Some(&w), true, Decision::Approve, 2),
            Ok(APPROVE_KEY)
        );
        for kind in [
            "elicitation_dialog",
            "elicitation_url_dialog",
            "idle_prompt",
            "session_start",
        ] {
            let a = attention(7, kind);
            assert_eq!(
                approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 7),
                Err(NOT_PERMISSION),
                "{kind}"
            );
        }
    }

    #[test]
    fn without_the_feature_only_seq_zero_passes() {
        assert_eq!(
            approve_key(NeedsApproval, None, false, Decision::Approve, 0),
            Ok(APPROVE_KEY)
        );
        assert_eq!(
            approve_key(NeedsApproval, None, false, Decision::Approve, 1),
            Err(STALE)
        );
        assert_eq!(
            approve_key(Idle, None, false, Decision::Approve, 0),
            Err(STALE)
        );
    }

    #[test]
    fn paste_is_bracketed_and_cannot_escape() {
        assert_eq!(paste_bytes("hi", false).unwrap(), b"\x1b[200~hi\x1b[201~");
        assert_eq!(paste_bytes("hi", true).unwrap(), b"\x1b[200~hi\x1b[201~\r");
        let evil = "a\x1b[201~\rrm -rf /\r\x03\x7f\u{9b}2J\tb\r\nc";
        let out = paste_bytes(evil, false).unwrap();
        assert_eq!(out, "\x1b[200~a\nrm -rf /\n2J\tb\nc\x1b[201~".as_bytes());
        let body = &out[PASTE_START.len()..out.len() - PASTE_END.len()];
        assert!(!body.contains(&0x1b) && !body.contains(&b'\r'));
        // Removing one end marker cannot assemble another.
        let nested = paste_bytes("\x1b[20\x1b[201~1~x", false).unwrap();
        let body = &nested[PASTE_START.len()..nested.len() - PASTE_END.len()];
        assert!(!body.contains(&0x1b), "{body:?}");
    }

    #[test]
    fn paste_has_a_size_cap() {
        assert!(paste_bytes(&"x".repeat(PASTE_MAX), false).is_ok());
        assert_eq!(
            paste_bytes(&"x".repeat(PASTE_MAX + 1), false),
            Err(TOO_LARGE)
        );
    }
}
