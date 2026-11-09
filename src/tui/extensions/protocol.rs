//! Protocol v1 with external extensions: the request lines claudio writes
//! and what it accepts back. Pure: the extension host (`host.rs`) moves the
//! lines; this module builds them and turns results into capped, sanitized
//! values. Every string a program returns is untrusted.

use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;

use super::{ActionResult, ListItem};
use crate::claude::latex::sanitize;

/// The protocol version in `CLAUDIO_EXTENSION_PROTOCOL`.
pub const VERSION: u32 = 1;
/// The longest line read from a program.
pub const MAX_LINE: usize = 1 << 20;
const INLINE_MAX_COLS: usize = 512;
const BLOCK_MAX_COLS: usize = 200;
const BLOCK_MAX_ROWS: usize = 100;
const MARKDOWN_MAX: usize = 256 * 1024;
const PASTE_MAX: usize = 64 * 1024;
const LIST_MAX: usize = 500;
const LABEL_MAX: usize = 200;
const DETAIL_MAX: usize = 400;
const NOTICE_MAX: usize = 200;
const TITLE_MAX: usize = 120;

/// A render result: its one-line form and its block, each optional.
pub type Rendering = (Option<String>, Option<Laid>);

/// A render result's block: its rows, top to bottom, each one line.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub rows: Vec<String>,
}

/// What a session an action runs for is, as the program is told.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionContext {
    pub host: String,
    pub cwd: String,
    pub name: Option<String>,
    pub conversation: Option<String>,
}

/// A request line (without its newline).
pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// `render`'s params.
pub fn render_params(lens: &str, kind: &str, source: &str) -> Value {
    json!({ "lens": lens, "kind": kind, "source": source })
}

/// `action`'s params.
pub fn action_params(action: &str, input: Option<&str>, session: Option<&SessionContext>) -> Value {
    let session = session.map(
        |s| json!({ "host": s.host, "cwd": s.cwd, "name": s.name, "conversation": s.conversation }),
    );
    json!({ "action": action, "input": input, "session": session })
}

/// One line from a program: the id it answers and its result or error.
/// `None` for a line that is valid JSON but no answer (ignored); `Err` for
/// one that is not JSON (a protocol error).
#[allow(clippy::type_complexity)]
pub fn reply(line: &[u8]) -> Result<Option<(u64, Result<Value, String>)>, String> {
    let v: Value = serde_json::from_slice(line).map_err(|e| format!("bad line: {e}"))?;
    let Some(id) = v.get("id").and_then(Value::as_u64) else {
        return Ok(None);
    };
    if let Some(result) = v.get("result") {
        return Ok(Some((id, Ok(result.clone()))));
    }
    let error = v.get("error").map(|e| match e {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    Ok(Some((
        id,
        Err(clip(&error.unwrap_or_default(), NOTICE_MAX)),
    )))
}

/// A render result: its one-line and block forms, each optional. `None`
/// (the item stays raw) when it breaks a cap or is not an object.
pub fn rendering(result: &Value) -> Option<Rendering> {
    let inline = match result.get("inline") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let line = one_line(s);
            if line.width() > INLINE_MAX_COLS {
                return None;
            }
            Some(line).filter(|l| !l.is_empty())
        }
        Some(_) => return None,
    };
    let block = match result.get("block") {
        None | Some(Value::Null) => None,
        Some(Value::Array(rows)) => {
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let row = one_line(row.as_str()?);
                if row.width() > BLOCK_MAX_COLS {
                    return None;
                }
                out.push(row);
            }
            if out.len() > BLOCK_MAX_ROWS {
                return None;
            }
            Some(Laid { rows: out }).filter(|l| !l.rows.is_empty())
        }
        Some(_) => return None,
    };
    result.is_object().then_some((inline, block))
}

/// An action result, capped and sanitized; `Err` is the notice to show
/// instead (a malformed result, or one too large).
pub fn action_result(result: &Value) -> Result<ActionResult, String> {
    action_result_in(result, true)
}

fn action_result_in(result: &Value, top: bool) -> Result<ActionResult, String> {
    let text = |key: &str| result.get(key).and_then(Value::as_str);
    match text("kind") {
        Some("document") => {
            let markdown = sanitize(text("markdown").unwrap_or_default());
            if markdown.len() > MARKDOWN_MAX {
                return Err("result too large".to_owned());
            }
            Ok(ActionResult::Document {
                title: clip(text("title").unwrap_or_default(), TITLE_MAX),
                markdown,
            })
        }
        Some("notice") => Ok(ActionResult::Notice {
            text: clip(text("text").unwrap_or_default(), NOTICE_MAX),
        }),
        Some("paste") => {
            let text = sanitize(text("text").unwrap_or_default());
            if text.len() > PASTE_MAX {
                return Err("result too large".to_owned());
            }
            Ok(ActionResult::Paste { text })
        }
        Some("list") if top => {
            let items = result.get("items").and_then(Value::as_array);
            let items = items
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let label = clip(item.get("label")?.as_str()?, LABEL_MAX);
                    let detail = item.get("detail").and_then(Value::as_str);
                    let result = action_result_in(item.get("result")?, false).ok()?;
                    Some(ListItem {
                        label,
                        detail: detail.map(|d| clip(d, DETAIL_MAX)),
                        result,
                    })
                })
                .take(LIST_MAX)
                .collect();
            Ok(ActionResult::List {
                title: clip(text("title").unwrap_or_default(), TITLE_MAX),
                items,
            })
        }
        _ => Err("the extension gave an unknown result".to_owned()),
    }
}

/// `s` sanitized on one line (tabs and newlines as spaces).
fn one_line(s: &str) -> String {
    sanitize(s).replace(['\n', '\t'], " ")
}

/// `s` on one line, cut to `max` chars on a char boundary.
fn clip(s: &str, max: usize) -> String {
    one_line(s).chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_one_json_line() {
        let line = request(7, "render", render_params("keys", "tag", "Ctrl+C\n"));
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["params"]["source"], "Ctrl+C\n");
        let session = SessionContext {
            host: "local".into(),
            cwd: "/p".into(),
            name: None,
            conversation: Some("c".into()),
        };
        let v = action_params("stamp", Some("x"), Some(&session));
        assert_eq!(v["session"]["host"], "local");
        assert_eq!(v["session"]["name"], Value::Null);
        assert_eq!(action_params("stamp", None, None)["session"], Value::Null);
    }

    #[test]
    fn replies_are_matched_by_id_and_bad_lines_are_protocol_errors() {
        let ok = reply(br#"{"id":3,"result":{"inline":"x"}}"#).unwrap();
        assert_eq!(ok, Some((3, Ok(json!({"inline": "x"})))));
        let err = reply(br#"{"id":4,"error":"no\u001b[2J"}"#).unwrap();
        assert_eq!(err, Some((4, Err("no[2J".to_owned()))));
        assert_eq!(reply(br#"{"hello":1}"#).unwrap(), None);
        assert!(reply(b"not json").is_err());
    }

    #[test]
    fn renderings_are_capped_and_sanitized() {
        let r = rendering(&json!({"inline": "⌃\u{1b}C\n", "block": ["┌─┐", "│\tC│"]})).unwrap();
        assert_eq!(r.0.as_deref(), Some("⌃C "));
        assert_eq!(r.1.unwrap().rows, ["┌─┐", "│ C│"]);
        assert_eq!(rendering(&json!({})), Some((None, None)));
        assert!(rendering(&json!({"inline": "x".repeat(513)})).is_none());
        assert!(rendering(&json!({"block": ["x".repeat(201)]})).is_none());
        let rows: Vec<String> = (0..101).map(|i| i.to_string()).collect();
        assert!(rendering(&json!({ "block": rows })).is_none());
        assert!(rendering(&json!({"inline": 3})).is_none());
        assert!(rendering(&json!("x")).is_none());
    }

    #[test]
    fn action_results_are_capped_and_sanitized() {
        let doc =
            action_result(&json!({"kind": "document", "title": "T\n", "markdown": "# a\u{7}"}));
        assert_eq!(
            doc,
            Ok(ActionResult::Document {
                title: "T ".into(),
                markdown: "# a".into()
            })
        );
        let paste = action_result(&json!({"kind": "paste", "text": "a\nb\u{1b}[201~"}));
        assert_eq!(
            paste,
            Ok(ActionResult::Paste {
                text: "a\nb[201~".into()
            })
        );
        let big = "x".repeat(64 * 1024 + 1);
        let too_big = action_result(&json!({"kind": "paste", "text": big}));
        assert_eq!(too_big, Err("result too large".to_owned()));
        let notice = action_result(&json!({"kind": "notice", "text": "n".repeat(300)}));
        assert_eq!(
            notice,
            Ok(ActionResult::Notice {
                text: "n".repeat(200)
            })
        );
        let items: Vec<Value> = (0..600)
            .map(|i| json!({"label": format!("l{i}"), "detail": "d", "result": {"kind": "paste", "text": "p"}}))
            .chain([json!({"label": "nested", "result": {"kind": "list", "items": []}})])
            .collect();
        let Ok(ActionResult::List { items, .. }) =
            action_result(&json!({"kind": "list", "title": "L", "items": items}))
        else {
            panic!("not a list")
        };
        assert_eq!(items.len(), 500);
        assert_eq!(items[0].detail.as_deref(), Some("d"));
        assert!(action_result(&json!({"kind": "shell"})).is_err());
    }
}
