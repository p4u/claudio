//! The browser's WebSocket (docs/web-protocol.md §2, §3): `hello` and the
//! snapshot on open, then requests (JSON text frames) and session input
//! (binary frames), each checked against the protocol's limits, and the
//! request ops of §5.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;

use super::hub::{daemon_error, HubHandle, Out, TabId, TabTx, MAX_TABS};
use super::input::Decision;
use super::spawn::{self, check_size, RespawnReq, SpawnReq};
use super::state::Layout;
use super::AppState;
use super::{addons, auth, ext, git_ops, sessions};
use claudio_core::paths;
use claudio_core::proto::{Msg, SearchHit, SessionId};

/// The largest client text frame: a 64 KiB text and its JSON envelope.
pub(crate) const MAX_TEXT: usize = 128 * 1024;
/// The largest input payload of a client binary frame (17 header bytes more).
pub(crate) const MAX_INPUT: usize = 64 * 1024;
/// Binary frame kind: client → server input (output frames, `hub::KIND_OUTPUT`,
/// are built by the hub).
pub(crate) const KIND_INPUT: u8 = 0x02;
/// Kind byte and session uuid.
const HEADER: usize = 17;

const CLOSE_TOO_BIG: u16 = 1009;
const CLOSE_BAD_FRAME: u16 = 1003;

/// Requests one tab may have in flight.
const IN_FLIGHT: usize = 64;
/// A search's deadline, counting the wait for the host's slot.
const SEARCH_DEADLINE: Duration = Duration::from_secs(10);

/// One of the [`MAX_TABS`] tab slots, held from the upgrade to the end of
/// the tab.
pub(crate) struct TabSlot(Arc<AtomicUsize>);

impl TabSlot {
    fn take(count: &Arc<AtomicUsize>) -> Option<TabSlot> {
        crate::atomic::increment_below(count, MAX_TABS).then(|| TabSlot(Arc::clone(count)))
    }
}

impl Drop for TabSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A tab's place in the hub and its writer, ended however the tab ends,
/// its future dropped included: the hub forgets the tab and the writer
/// stops, which drops the socket.
struct TabEnd {
    hub: HubHandle,
    tab: TabId,
    writer: tokio::task::AbortHandle,
}

impl Drop for TabEnd {
    fn drop(&mut self) {
        self.hub.close_tab(self.tab);
        self.writer.abort();
    }
}

/// `GET /ws` (the guard checked Host, Origin and the cookie). While the
/// tab is open, its session counts as used (`sessions::OpenTab`); when the
/// session is gone (`Sessions::gone`), the tab is dropped, and its socket
/// and `TabSlot` with it.
pub(crate) async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(slot) = TabSlot::take(&state.tabs) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many tabs").into_response();
    };
    let session = auth::cookie_value(&headers).and_then(|v| sessions::hash_of(&v));
    ws.max_message_size(2 << 20)
        .max_frame_size(2 << 20)
        .on_upgrade(move |socket| async move {
            let Some(hash) = session else {
                return tab(state, socket, slot).await;
            };
            let kept = Arc::clone(&state.auth.sessions);
            let _open = sessions::OpenTab::start(Arc::clone(&kept), hash, paths::unix_now);
            tokio::select! {
                () = tab(state, socket, slot) => {}
                () = kept.gone(&hash) => {}
            }
        })
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

/// One browser tab: `hello`, then the hub's snapshot and replay, then a
/// writer draining the tab's queue while this task reads its frames.
async fn tab(state: AppState, socket: WebSocket, _slot: TabSlot) {
    let (tx, mut rx) = TabTx::new();
    let hello = json!({
        "t": "hello",
        "v": 2,
        "version": super::version(),
        "features": [],
    });
    tx.send(Out::Text(hello.to_string()));
    let (mut sink, mut stream) = socket.split();
    let tab = match state.hub.open_tab(tx.clone()).await {
        Ok(tab) => tab,
        Err(why) => {
            let close = CloseFrame {
                code: 1013,
                reason: why.into(),
            };
            let _ = sink.send(Message::Close(Some(close))).await;
            return;
        }
    };
    let drained = tx.clone();
    let mut writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let n = out.size();
            let sent = match out {
                Out::Text(text) => sink.send(Message::Text(text.into())).await,
                Out::Binary(bytes) => sink.send(Message::Binary(bytes)).await,
                Out::Close(code, reason) => {
                    let close = CloseFrame {
                        code,
                        reason: reason.into(),
                    };
                    let _ = sink.send(Message::Close(Some(close))).await;
                    break;
                }
            };
            drained.sent(n);
            if sent.is_err() {
                break;
            }
        }
    });
    let _end = TabEnd {
        hub: state.hub.clone(),
        tab,
        writer: writer.abort_handle(),
    };
    let slots = Arc::new(Semaphore::new(IN_FLIGHT));
    let mut closing = false;
    loop {
        let msg = tokio::select! {
            msg = stream.next() => msg,
            _ = &mut writer => break,
        };
        let Some(Ok(msg)) = msg else { break };
        let frame = match &msg {
            Message::Text(text) => match text_frame(text.as_str()) {
                Frame::Request { id, op } => {
                    let args = request_args(text.as_str());
                    match Arc::clone(&slots).try_acquire_owned() {
                        Ok(permit) => {
                            let (state, tx) = (state.clone(), tx.clone());
                            tokio::spawn(async move {
                                let res = dispatch(&state, tab, &op, args).await;
                                tx.send(Out::Text(reply(id, res).to_string()));
                                drop(permit);
                            });
                        }
                        Err(_) => {
                            tx.send(Out::Text(reply(id, Err("busy".into())).to_string()));
                        }
                    }
                    continue;
                }
                other => other,
            },
            Message::Binary(bytes) => match binary_frame(bytes) {
                Frame::Input => {
                    if let Ok(sid) = SessionId::from_slice(&bytes[1..HEADER]) {
                        state.hub.input(sid, bytes[HEADER..].to_vec());
                    }
                    continue;
                }
                other => other,
            },
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        if let Frame::Close(code, reason) = frame {
            closing = tx.send(Out::Close(code, reason));
            break;
        }
    }
    drop(tx);
    if closing {
        // Let the writer flush the close frame (`_end` then stops it).
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut writer).await;
    }
}

/// A request's args: its top-level keys but `t`, `id` and `op`.
fn request_args(text: &str) -> Value {
    let mut obj: Map<String, Value> = serde_json::from_str(text).unwrap_or_default();
    for key in ["t", "id", "op"] {
        obj.remove(key);
    }
    Value::Object(obj)
}

fn reply(id: u32, res: Result<Value, String>) -> Value {
    match res {
        Ok(data) => json!({"t": "res", "id": id, "ok": true, "data": data}),
        Err(error) => json!({"t": "res", "id": id, "ok": false, "error": error}),
    }
}

/// `args` as an op's argument struct; `bad request: <why>` otherwise.
pub(super) fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad request: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostArgs {
    host: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SidArgs {
    pub(super) sid: SessionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeArgs {
    sid: SessionId,
    rows: u16,
    cols: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellArgs {
    host: String,
    cwd: String,
    rows: u16,
    cols: u16,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameArgs {
    sid: SessionId,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasteArgs {
    sid: SessionId,
    text: String,
    #[serde(default)]
    submit: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveArgs {
    sid: SessionId,
    decision: Decision,
    attention_seq: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderArgs {
    sids: Vec<SessionId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    host: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectsArgs {
    host: String,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationsArgs {
    host: String,
    cwd: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeUpdateArgs {
    host: String,
    #[serde(default)]
    install: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutArgs {
    layout: Layout,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToggleArgs {
    name: String,
    on: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionArgs {
    ext: String,
    action: String,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    sid: Option<SessionId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    lens: String,
    kind: String,
    source: String,
}

/// The longest path `fs.list` / `claude.sessions` take, in bytes.
const PATH_MAX: usize = 4096;
/// The longest search query, in bytes.
const QUERY_MAX: usize = 1024;

pub(super) fn check_path(field: &str, path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > PATH_MAX {
        return Err(format!(
            "bad request: {field}: must be 1..={PATH_MAX} bytes"
        ));
    }
    Ok(())
}

/// A daemon reply as the browser gets it.
pub(super) fn to_json<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|_| "cannot encode the reply".to_owned())
}

pub(super) const UNEXPECTED: &str = "unexpected reply from the daemon";

/// One request op (docs/web-protocol.md §5).
pub(crate) async fn dispatch(
    state: &AppState,
    tab: TabId,
    op: &str,
    args: Value,
) -> Result<Value, String> {
    let hub = &state.hub;
    match op {
        "hosts.list" => {
            parse::<NoArgs>(args)?;
            Ok(json!({ "hosts": hub.hosts().await }))
        }
        "hosts.candidates" => {
            parse::<NoArgs>(args)?;
            let connected: Vec<String> = hub
                .hosts()
                .await
                .into_iter()
                .filter(|h| h["connected"] == true)
                .filter_map(|h| h["name"].as_str().map(str::to_owned))
                .collect();
            let all = tokio::task::spawn_blocking(claudio_core::remote::hosts::candidates)
                .await
                .unwrap_or_default();
            let hosts: Vec<String> = all.into_iter().filter(|h| !connected.contains(h)).collect();
            Ok(json!({ "hosts": hosts }))
        }
        "hosts.connect" => {
            let a: HostArgs = parse(args)?;
            hub.connect(a.host).await
        }
        "hosts.disconnect" => {
            let a: HostArgs = parse(args)?;
            hub.disconnect(a.host).await.map(|()| json!({}))
        }
        "session.attach" => {
            let a: SizeArgs = parse(args)?;
            check_size(a.rows, a.cols)?;
            hub.attach(tab, a.sid, a.rows, a.cols)
                .await
                .map(|()| json!({}))
        }
        "session.detach" => {
            let a: SidArgs = parse(args)?;
            hub.detach(tab, a.sid).await.map(|()| json!({}))
        }
        "session.resize" => {
            let a: SizeArgs = parse(args)?;
            check_size(a.rows, a.cols)?;
            hub.resize(a.sid, a.rows, a.cols).await.map(|()| json!({}))
        }
        "session.spawn" => spawn::spawn_claude(&state.ctx, parse::<SpawnReq>(args)?).await,
        "session.shell" => {
            let a: ShellArgs = parse(args)?;
            let sid = spawn::spawn_shell(&state.ctx, a.host, a.cwd, a.name, a.rows, a.cols).await?;
            Ok(json!({ "sid": sid }))
        }
        "session.respawn" => {
            let req: RespawnReq = parse(args)?;
            let sid = spawn::respawn(&state.ctx, req).await?;
            Ok(json!({ "sid": sid }))
        }
        "session.kill" => {
            let a: SidArgs = parse(args)?;
            let s = hub.session(a.sid).await?;
            s.client
                .request(Msg::Kill { id: a.sid })
                .await
                .map_err(daemon_error)?;
            tracing::info!(op = "session.kill", host = %s.host, sid = %a.sid, "web op");
            Ok(json!({}))
        }
        "session.rename" => {
            let a: RenameArgs = parse(args)?;
            let name = spawn::check_name(a.name.as_deref())?;
            let s = hub.session(a.sid).await?;
            s.client
                .request(Msg::Rename { id: a.sid, name })
                .await
                .map_err(daemon_error)?;
            Ok(json!({}))
        }
        "session.paste" => {
            let a: PasteArgs = parse(args)?;
            hub.paste(a.sid, a.text, a.submit).await.map(|()| json!({}))
        }
        "session.approve" => {
            let a: ApproveArgs = parse(args)?;
            hub.approve(a.sid, a.decision, a.attention_seq)
                .await
                .map(|()| json!({}))
        }
        "session.order" => {
            let a: OrderArgs = parse(args)?;
            if a.sids.len() > 4096 {
                return Err("bad request: sids: at most 4096".to_owned());
            }
            hub.order(tab, a.sids).await;
            Ok(json!({}))
        }
        "fs.list" => {
            let a: ListArgs = parse(args)?;
            check_path("path", &a.path)?;
            let (client, _) = hub.host_client(&a.host).await?;
            match client
                .request(Msg::ListDir { path: a.path })
                .await
                .map_err(daemon_error)?
            {
                Msg::DirEntries {
                    path,
                    entries,
                    truncated,
                } => {
                    Ok(json!({"path": path, "entries": to_json(entries)?, "truncated": truncated}))
                }
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "fs.projects" => {
            let a: ProjectsArgs = parse(args)?;
            let limit = a.limit.unwrap_or(50);
            if !(1..=500).contains(&limit) {
                return Err("bad request: limit: must be 1..=500".to_owned());
            }
            let (client, _) = hub.host_client(&a.host).await?;
            match client
                .request(Msg::RecentProjects { limit })
                .await
                .map_err(daemon_error)?
            {
                Msg::Projects { dirs } => Ok(json!({ "dirs": to_json(dirs)? })),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "claude.sessions" => {
            let a: ConversationsArgs = parse(args)?;
            check_path("cwd", &a.cwd)?;
            let (client, _) = hub.host_client(&a.host).await?;
            match client
                .request(Msg::ListClaudeSessions { cwd: a.cwd })
                .await
                .map_err(daemon_error)?
            {
                Msg::ClaudeSessions { cwd, mut sessions } => {
                    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
                    Ok(json!({"cwd": cwd, "sessions": to_json(sessions)?}))
                }
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "reader.answers" => {
            let a: SidArgs = parse(args)?;
            let s = hub.session(a.sid).await?;
            match s
                .client
                .request(Msg::Answers { id: a.sid })
                .await
                .map_err(daemon_error)?
            {
                Msg::AnswersReply { mut answers, .. } => {
                    answers.reverse();
                    Ok(json!({ "answers": to_json(answers)? }))
                }
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "reader.last_prompt" => {
            let a: SidArgs = parse(args)?;
            let s = hub.session(a.sid).await?;
            match s
                .client
                .request(Msg::LastPrompt { id: a.sid })
                .await
                .map_err(daemon_error)?
            {
                Msg::LastPromptReply { prompt, cut, .. } => {
                    Ok(json!({"prompt": prompt, "cut": cut}))
                }
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "search" => {
            let a: SearchArgs = parse(args)?;
            if a.query.trim().is_empty() || a.query.len() > QUERY_MAX {
                return Err(format!("bad request: query: must be 1..={QUERY_MAX} bytes"));
            }
            Ok(search(state, a.query).await)
        }
        op if op.starts_with("git.") => git_ops::dispatch(hub, op, args).await,
        "claude.update" => {
            let a: ClaudeUpdateArgs = parse(args)?;
            hub.claude_update(a.host, a.install)
                .await
                .map(|()| json!({}))
        }
        "claude.skip" => {
            let a: HostArgs = parse(args)?;
            hub.claude_skip(a.host).await;
            Ok(json!({}))
        }
        "layout.save" => {
            let a: LayoutArgs = parse(args)?;
            hub.layout(tab, a.layout).await.map(|()| json!({}))
        }
        op if op.starts_with("snippets.") => ext::snippets(op, args).await,
        "extensions.list" => {
            parse::<NoArgs>(args)?;
            Ok(state.ctx.ext.list())
        }
        "extensions.toggle" => {
            let a: ToggleArgs = parse(args)?;
            let ctx = &state.ctx;
            ctx.ext
                .toggle(&state.addons, hub, &a.name, a.on)
                .await
                .map(|()| json!({}))
        }
        "extensions.action" => {
            let a: ActionArgs = parse(args)?;
            state
                .ctx
                .ext
                .action(hub, &a.ext, &a.action, a.input, a.sid)
                .await
        }
        "extensions.render" => {
            let a: RenderArgs = parse(args)?;
            state.ctx.ext.render(&a.lens, &a.kind, &a.source).await
        }
        op if op.starts_with("addons.") => addons::dispatch(state, op, args).await,
        _ => Err("unknown op".to_owned()),
    }
}

/// `search` (docs/web-protocol.md §5): every connected host with the
/// `search` feature, one request per host at a time, 10 s counting the
/// wait, one more try 500 ms after `search busy`; hits newest first.
async fn search(state: &AppState, query: String) -> Value {
    let hosts: Vec<String> = state
        .hub
        .hosts()
        .await
        .into_iter()
        .filter(|h| h["connected"] == true)
        .filter(|h| {
            h["features"]
                .as_array()
                .is_some_and(|f| f.iter().any(|x| x == "search"))
        })
        .filter_map(|h| h["name"].as_str().map(str::to_owned))
        .collect();
    let asks = hosts.into_iter().map(|host| {
        let (state, query) = (state.clone(), query.clone());
        async move {
            let res = tokio::time::timeout(SEARCH_DEADLINE, search_host(&state, &host, query))
                .await
                .unwrap_or_else(|_| Err("timed out".to_owned()));
            (host, res)
        }
    });
    let mut hits: Vec<(String, SearchHit)> = Vec::new();
    let mut failed = Vec::new();
    let mut truncated = false;
    for (host, res) in futures::future::join_all(asks).await {
        match res {
            Ok((found, cut)) => {
                truncated |= cut;
                hits.extend(found.into_iter().map(|h| (host.clone(), h)));
            }
            Err(error) => failed.push(json!({"host": host, "error": error})),
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.1.modified));
    let hits: Vec<Value> = hits
        .into_iter()
        .map(|(host, h)| {
            json!({
                "host": host,
                "conversation": h.conversation,
                "cwd": h.cwd,
                "modified": h.modified,
                "title": h.title,
                "snippet": h.snippet,
                "matches": h.matches,
            })
        })
        .collect();
    json!({"hits": hits, "failed": failed, "truncated": truncated})
}

async fn search_host(
    state: &AppState,
    host: &str,
    query: String,
) -> Result<(Vec<SearchHit>, bool), String> {
    let slot = {
        let mut slots = state.search_slots.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(
            slots
                .entry(host.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(1))),
        )
    };
    let _permit = slot
        .acquire_owned()
        .await
        .map_err(|_| "closed".to_owned())?;
    let (client, _) = state.hub.host_client(host).await?;
    let ask = || {
        client.request(Msg::Search {
            query: query.clone(),
            limit: 0,
        })
    };
    let mut reply = ask().await;
    if matches!(&reply, Err(e) if e.to_string() == "search busy") {
        tokio::time::sleep(Duration::from_millis(500)).await;
        reply = ask().await;
    }
    match reply.map_err(daemon_error)? {
        Msg::SearchReply {
            hits, truncated, ..
        } => Ok((hits, truncated)),
        _ => Err(UNEXPECTED.to_owned()),
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
        // A 64 KiB paste fits with its envelope (Ruling R19).
        let paste = format!(
            r#"{{"t":"req","id":2,"op":"session.paste","sid":"x","text":"{}","submit":false}}"#,
            "x".repeat(64 * 1024 + 1)
        );
        assert_eq!(
            text_frame(&paste),
            Frame::Request {
                id: 2,
                op: "session.paste".into()
            }
        );
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
            binary_frame(&[crate::hub::KIND_OUTPUT; HEADER]),
            Frame::Close(1003, "bad frame")
        );
        assert_eq!(
            binary_frame(&vec![KIND_INPUT; HEADER + MAX_INPUT + 1]),
            Frame::Close(1009, "too big")
        );
    }
}
