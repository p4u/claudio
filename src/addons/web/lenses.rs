//! The lens relay: the tag-lens items a daemon sends for a session (its
//! `Lens` events, or a pre-lens daemon's `Latex` formulas as `latex`
//! items), kept in the session's view and sent to the page as `lens`
//! events with their `latex` twin (docs/web-protocol.md).

use serde_json::{json, Value};

use super::events::ev;
use super::hub::SessionView;
use crate::proto::{LensItem, SessionEvent, SessionId};

/// A `Latex` or `Lens` event applied to `view`: the list to send, if it is
/// new. A daemon without the `lens` feature (`lens_host`) sends formulas
/// only, which are its `latex` items; a lens daemon's are in its `Lens`
/// events. Any other event is ignored.
pub(crate) fn relay(
    view: &mut SessionView,
    event: SessionEvent,
    lens_host: bool,
) -> Option<(u64, Vec<LensItem>)> {
    match event {
        SessionEvent::Latex { seq, formulas } if !lens_host => {
            let items = formulas
                .into_iter()
                .map(|text| LensItem {
                    tag: "latex".to_owned(),
                    text,
                })
                .collect();
            take_lens(view, seq, items)
        }
        SessionEvent::Lens { seq, items } if lens_host => take_lens(view, seq, items),
        _ => None,
    }
}

/// Take lens list `seq`, unless a newer one is held (a replay and a
/// broadcast may cross).
fn take_lens(
    view: &mut SessionView,
    seq: u64,
    items: Vec<LensItem>,
) -> Option<(u64, Vec<LensItem>)> {
    if seq <= view.lens_seq {
        return None;
    }
    view.lens_seq = seq;
    view.lens = items.clone();
    Some((seq, items))
}

/// The `lens` event of a list and its `latex` twin (same seq).
pub(crate) fn lens_events(sid: SessionId, seq: u64, items: &[LensItem]) -> [Value; 2] {
    let formulas: Vec<&str> = items
        .iter()
        .filter(|i| i.tag == "latex")
        .map(|i| i.text.as_str())
        .collect();
    [
        ev("lens", json!({"sid": sid, "seq": seq, "items": items})),
        ev(
            "latex",
            json!({"sid": sid, "seq": seq, "formulas": formulas}),
        ),
    ]
}
