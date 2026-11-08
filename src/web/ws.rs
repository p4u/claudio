//! The browser's WebSocket (docs/web-protocol.md §2, §3): `hello` and the
//! snapshot on open, then requests (JSON text frames) and session input
//! (binary frames), each checked against the protocol's limits.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use super::AppState;

/// The largest client text frame.
pub(crate) const MAX_TEXT: usize = 64 * 1024;
/// The largest input payload of a client binary frame (17 header bytes more).
pub(crate) const MAX_INPUT: usize = 64 * 1024;
/// Binary frame kinds: server → client PTY output, client → server input.
#[allow(dead_code)] // sent by the hub (Task 4)
pub(crate) const KIND_OUTPUT: u8 = 0x01;
pub(crate) const KIND_INPUT: u8 = 0x02;
/// Kind byte and session uuid.
const HEADER: usize = 17;

const CLOSE_TOO_BIG: u16 = 1009;
const CLOSE_BAD_FRAME: u16 = 1003;

/// `GET /ws` (the guard checked Host, Origin and the cookie).
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(2 << 20)
        .max_frame_size(2 << 20)
        .on_upgrade(move |socket| tab(state, socket))
}

/// What a client frame asks for.
#[derive(Debug, PartialEq)]
enum Frame {
    Request { id: u32, op: String },
    Input,
    Close(u16, &'static str),
}

/// Check a text frame: a JSON object with `"t":"req"`, a `u32` `id` and a
/// string `op`.
fn text_frame(text: &str) -> Frame {
    if text.len() > MAX_TEXT {
        return Frame::Close(CLOSE_TOO_BIG, "too big");
    }
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
        return Frame::Close(CLOSE_BAD_FRAME, "bad frame");
    };
    let id = obj
        .get("id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok());
    match (
        obj.get("t").and_then(Value::as_str),
        id,
        obj.get("op").and_then(Value::as_str),
    ) {
        (Some("req"), Some(id), Some(op)) => Frame::Request {
            id,
            op: op.to_owned(),
        },
        _ => Frame::Close(CLOSE_BAD_FRAME, "bad frame"),
    }
}

/// Check a binary frame: kind `0x02`, a session uuid, ≤ [`MAX_INPUT`].
fn binary_frame(bytes: &[u8]) -> Frame {
    if bytes.len() > MAX_INPUT + HEADER {
        Frame::Close(CLOSE_TOO_BIG, "too big")
    } else if bytes.len() < HEADER || bytes[0] != KIND_INPUT {
        Frame::Close(CLOSE_BAD_FRAME, "bad frame")
    } else {
        Frame::Input
    }
}

async fn send_json(socket: &mut WebSocket, value: Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

/// One browser tab. Task 4 replaces the empty snapshot and the request
/// dispatch with the hub's.
async fn tab(_state: AppState, mut socket: WebSocket) {
    let hello = json!({
        "t": "hello",
        "v": 1,
        "version": env!("CARGO_PKG_VERSION"),
        "features": [],
    });
    let snapshot = json!({
        "t": "snapshot",
        "hosts": [],
        "sessions": [],
        "profiles": {"names": [], "default": null},
        "layout": {"grid": 1, "panes": [null], "focus": 0},
        "order": [],
    });
    if !send_json(&mut socket, hello).await || !send_json(&mut socket, snapshot).await {
        return;
    }
    while let Some(Ok(msg)) = socket.recv().await {
        let frame = match &msg {
            Message::Text(text) => text_frame(text.as_str()),
            Message::Binary(bytes) => binary_frame(bytes),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        match frame {
            Frame::Request { id, .. } => {
                let res = json!({"t": "res", "id": id, "ok": false, "error": "unknown op"});
                if !send_json(&mut socket, res).await {
                    break;
                }
            }
            Frame::Input => {}
            Frame::Close(code, reason) => {
                let close = CloseFrame {
                    code,
                    reason: reason.into(),
                };
                let _ = socket.send(Message::Close(Some(close))).await;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_frames_must_be_requests_within_the_limit() {
        assert_eq!(
            text_frame(r#"{"t":"req","id":7,"op":"session.kill","sid":"x"}"#),
            Frame::Request {
                id: 7,
                op: "session.kill".into()
            }
        );
        for bad in [
            "not json",
            "[1]",
            r#"{"t":"res","id":1,"op":"x"}"#,
            r#"{"t":"req","op":"x"}"#,
            r#"{"t":"req","id":-1,"op":"x"}"#,
            r#"{"t":"req","id":4294967296,"op":"x"}"#,
            r#"{"t":"req","id":1,"op":2}"#,
        ] {
            assert_eq!(text_frame(bad), Frame::Close(1003, "bad frame"), "{bad}");
        }
        let big = format!(r#"{{"t":"req","id":1,"op":"{}"}}"#, "x".repeat(MAX_TEXT));
        assert_eq!(text_frame(&big), Frame::Close(1009, "too big"));
    }

    #[test]
    fn binary_frames_must_be_input_within_the_limit() {
        assert_eq!(binary_frame(&[KIND_INPUT; HEADER]), Frame::Input);
        assert_eq!(
            binary_frame(&vec![KIND_INPUT; HEADER + MAX_INPUT]),
            Frame::Input
        );
        assert_eq!(
            binary_frame(&[KIND_INPUT; HEADER - 1]),
            Frame::Close(1003, "bad frame")
        );
        assert_eq!(
            binary_frame(&[KIND_OUTPUT; HEADER]),
            Frame::Close(1003, "bad frame")
        );
        assert_eq!(
            binary_frame(&vec![KIND_INPUT; HEADER + MAX_INPUT + 1]),
            Frame::Close(1009, "too big")
        );
    }
}
