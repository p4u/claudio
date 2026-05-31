//! Persistent-session pool with continuation detection.
//!
//! OpenAI chat-completions is stateless: the client resends the whole
//! `messages[]` every turn. Spawning a fresh `claude` per request pays a
//! cold-start each time and re-processes the entire history. Instead we keep a
//! pool of live [`PtySession`]s and feed each one only the *delta*.
//!
//! **Matching.** Each live session remembers the messages it has served as a
//! cumulative prefix-hash chain `(prefix_hash, served_len)`. A new request
//! matches a session when the session's served messages are an exact prefix of
//! the request — i.e. `session.prefix_hash == request_cumhash[served_len]`. On a
//! match we send `messages[served_len..]` (skipping the client's echo of the
//! assistant's own prior turns); otherwise we start a fresh session with the
//! full conversation. The system/agentic prompt is re-injected every N turns.
//!
//! All of this is logged at INFO so the prefix-hash match and context reuse are
//! observable.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::WrapperEnv;
use crate::driver::PtySession;

use super::backend::RawResult;
use super::error::{AppError, AppResult};
use super::types::Message;
use super::usage::CliUsage;

/// Pool tuning, read from the environment once.
struct PoolCfg {
    /// Max conversation *mappings* kept (each resumable from disk).
    max_sessions: usize,
    /// Max *live* claude processes at once; idle ones are demoted to dormant.
    max_live: usize,
    /// Drop a mapping entirely after this much idle time.
    ttl: Duration,
    reinject_turns: u32,
}

impl PoolCfg {
    fn from_env() -> Self {
        let g = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        PoolCfg {
            max_sessions: g("CLAUDIO_API_MAX_SESSIONS", 32) as usize,
            max_live: g("CLAUDIO_API_MAX_LIVE", 6) as usize,
            ttl: Duration::from_secs(g("CLAUDIO_API_SESSION_TTL", 600)),
            reinject_turns: g("CLAUDIO_API_REINJECT_TURNS", 6) as u32,
        }
    }
}

/// One conversation's claude session: a persisted session id (context lives on
/// disk in claude's transcript) plus an *optional* live process. When the
/// process is demoted to save resources, `live` is `None`; the next turn revives
/// it with `--resume <csid>`, reloading the conversation and sending only the delta.
struct Slot {
    csid: String,
    model: String,
    live: Option<PtySession>,
}

impl Slot {
    fn is_live(&self) -> bool {
        self.live.as_ref().map(|s| s.is_alive()).unwrap_or(false)
    }

    /// Kill the live process (if any); the conversation stays resumable on disk.
    fn demote(&mut self) {
        if let Some(mut s) = self.live.take() {
            s.close();
        }
    }

    /// Ensure a live process (resuming the persisted session if dormant/dead),
    /// then run one turn.
    fn turn(
        &mut self,
        env: &WrapperEnv,
        base_forward: &[String],
        prompt: &str,
    ) -> Result<(crate::session::Summary, Option<String>), crate::driver::DriverError> {
        if !self.is_live() {
            self.live = None; // drop any dead session
            let mut forward = vec![
                "--model".to_string(),
                self.model.clone(),
                "--resume".to_string(),
                self.csid.clone(),
            ];
            forward.extend(base_forward.iter().cloned());
            tracing::info!(session = short(&self.csid), "RESUME — reviving dormant session");
            let sess = PtySession::start(env, &forward, &self.csid, None)?;
            self.live = Some(sess);
        }
        self.live.as_mut().unwrap().turn(prompt)
    }
}

/// Per-conversation bookkeeping kept under the pool lock (cheap to read for
/// matching; the slot itself is locked only during a turn).
struct Handle {
    csid: String,
    prefix_hash: u64,
    served_len: usize,
    turns: u32,
    last_used: Instant,
    slot: Arc<Mutex<Slot>>,
}

struct Inner {
    handles: Vec<Handle>,
}

/// A pool of persistent claude conversations shared across requests.
pub struct SessionPool {
    inner: Mutex<Inner>,
    cfg: PoolCfg,
    /// Flags every session's `claude` is launched with (no tools, no system
    /// flag — the system prompt rides in the typed prompt, unscored).
    base_forward: Vec<String>,
}

impl SessionPool {
    pub fn new(setting_sources: &str) -> Self {
        let mut base_forward: Vec<String> = vec![
            "--tools".into(),
            String::new(),
            "--strict-mcp-config".into(),
            "--disable-slash-commands".into(),
        ];
        if !setting_sources.is_empty() {
            base_forward.push("--setting-sources".into());
            base_forward.push(setting_sources.to_string());
        }
        SessionPool {
            inner: Mutex::new(Inner { handles: Vec::new() }),
            cfg: PoolCfg::from_env(),
            base_forward,
        }
    }

    /// Resolve one API turn: reuse a matching conversation (delta-only, reviving
    /// it from disk if its process was demoted) or start a fresh one (full
    /// context). Runs synchronously; call from `spawn_blocking`.
    pub fn resolve_turn(
        &self,
        env: &WrapperEnv,
        model: &str,
        messages: &[Message],
        system: &str,
        full_body: &str,
    ) -> AppResult<RawResult> {
        let cum = cumulative_hashes(messages);
        let req_hash = *cum.last().unwrap_or(&0);
        tracing::info!(
            messages = messages.len(),
            req_hash = format!("{req_hash:016x}"),
            "resolve_turn"
        );

        // Find the conversation whose served messages are an exact prefix of this
        // request (longest prefix = most context reused).
        let matched: Option<(Arc<Mutex<Slot>>, String, usize, u32)> = {
            let mut inner = self.inner.lock().unwrap();
            self.evict_expired(&mut inner);
            let mut best: Option<usize> = None;
            for (i, h) in inner.handles.iter().enumerate() {
                if h.served_len == 0 || h.served_len > messages.len() {
                    continue;
                }
                if cum[h.served_len] == h.prefix_hash
                    && best.map(|b| h.served_len > inner.handles[b].served_len).unwrap_or(true)
                {
                    best = Some(i);
                }
            }
            best.map(|i| {
                let h = &inner.handles[i];
                (h.slot.clone(), h.csid.clone(), h.served_len, h.turns)
            })
        };

        if let Some((slot, csid, served_len, turns)) = matched {
            let delta = &messages[served_len..];
            let reinject = turns > 0 && turns % self.cfg.reinject_turns == 0;
            let dormant = !slot.lock().unwrap().is_live();
            tracing::info!(
                session = short(&csid),
                reused = served_len,
                delta = delta.len(),
                turn = turns + 1,
                reinject,
                dormant,
                "MATCH (continuation) — sending delta only"
            );
            let prompt = build_delta_prompt(delta, system, reinject);

            // Make room for a live process (this conversation will need one).
            self.enforce_live_cap(&csid);

            let mut s = slot.lock().unwrap();
            let res = s.turn(env, &self.base_forward, &prompt);
            drop(s);

            match res {
                Ok((summary, failure)) => {
                    self.update_handle(&csid, req_hash, messages.len());
                    shape(summary, failure)
                }
                Err(e) => {
                    self.remove_handle(&csid);
                    Err(map_turn_err(e))
                }
            }
        } else {
            tracing::info!("NEW (no prefix match) — starting fresh session");
            self.start_fresh(env, model, messages, system, full_body, req_hash)
        }
    }

    fn start_fresh(
        &self,
        env: &WrapperEnv,
        model: &str,
        messages: &[Message],
        system: &str,
        full_body: &str,
        req_hash: u64,
    ) -> AppResult<RawResult> {
        self.evict_to_capacity();
        let csid = uuid::Uuid::new_v4().to_string();
        self.enforce_live_cap(&csid);

        let mut forward = vec!["--model".to_string(), model.to_string()];
        forward.extend(self.base_forward.iter().cloned());
        let mut session = PtySession::start(env, &forward, &csid, None)
            .map_err(|e| AppError::Internal(format!("failed to start claude session: {e}")))?;

        let prompt = build_full_prompt(system, full_body);
        let res = session.turn(&prompt);

        match res {
            Ok((summary, failure)) => {
                let handle = Handle {
                    csid: csid.clone(),
                    prefix_hash: req_hash,
                    served_len: messages.len(),
                    turns: 1,
                    last_used: Instant::now(),
                    slot: Arc::new(Mutex::new(Slot {
                        csid: csid.clone(),
                        model: model.to_string(),
                        live: Some(session),
                    })),
                };
                let mut inner = self.inner.lock().unwrap();
                inner.handles.push(handle);
                let (n, live) = (inner.handles.len(), self.live_count(&inner));
                drop(inner);
                tracing::info!(session = short(&csid), pool = n, live, served = messages.len(), "NEW session registered");
                shape(summary, failure)
            }
            Err(e) => {
                session.close();
                Err(map_turn_err(e))
            }
        }
    }

    fn live_count(&self, inner: &Inner) -> usize {
        inner
            .handles
            .iter()
            .filter(|h| h.slot.try_lock().map(|s| s.is_live()).unwrap_or(true))
            .count()
    }

    /// Demote least-recently-used live sessions (kill the process, keep the
    /// mapping) until there's room for one more, leaving `keep` alone.
    fn enforce_live_cap(&self, keep: &str) {
        let inner = self.inner.lock().unwrap();
        loop {
            // Idle (non-busy) live handles other than `keep`, oldest first.
            let mut live: Vec<(usize, Instant)> = inner
                .handles
                .iter()
                .enumerate()
                .filter(|(_, h)| h.csid != keep)
                .filter_map(|(i, h)| h.slot.try_lock().ok().filter(|s| s.is_live()).map(|_| (i, h.last_used)))
                .collect();
            if live.len() < self.cfg.max_live {
                break;
            }
            live.sort_by_key(|(_, t)| *t);
            let (idx, _) = live[0];
            let csid = inner.handles[idx].csid.clone();
            if let Ok(mut s) = inner.handles[idx].slot.try_lock() {
                s.demote();
                tracing::info!(session = short(&csid), "demoting LRU live session (process killed, context kept on disk)");
            } else {
                break;
            }
        }
    }

    /// Update a conversation's served prefix after a successful turn.
    fn update_handle(&self, csid: &str, prefix_hash: u64, served_len: usize) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(h) = inner.handles.iter_mut().find(|h| h.csid == csid) {
            h.prefix_hash = prefix_hash;
            h.served_len = served_len;
            h.turns += 1;
            h.last_used = Instant::now();
        }
    }

    fn remove_handle(&self, csid: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.handles.retain(|h| h.csid != csid);
    }

    /// Drop mappings idle past the TTL (their live process, if any, is killed).
    fn evict_expired(&self, inner: &mut Inner) {
        let now = Instant::now();
        let ttl = self.cfg.ttl;
        inner.handles.retain(|h| {
            let fresh = now.duration_since(h.last_used) < ttl;
            if !fresh {
                tracing::info!(session = short(&h.csid), "evicting session (idle TTL)");
            }
            fresh
        });
    }

    /// Evict the least-recently-used mapping(s) until under the mapping cap.
    fn evict_to_capacity(&self) {
        let mut inner = self.inner.lock().unwrap();
        self.evict_expired(&mut inner);
        while inner.handles.len() >= self.cfg.max_sessions {
            let idx = inner
                .handles
                .iter()
                .enumerate()
                .filter(|(_, h)| h.slot.try_lock().is_ok())
                .min_by_key(|(_, h)| h.last_used)
                .map(|(i, _)| i);
            match idx {
                Some(i) => {
                    let h = inner.handles.remove(i);
                    tracing::info!(session = short(&h.csid), "evicting LRU mapping (capacity)");
                }
                None => break,
            }
        }
    }
}

/// Convert a driver [`crate::session::Summary`] into a [`RawResult`].
fn shape(summary: crate::session::Summary, failure: Option<String>) -> AppResult<RawResult> {
    if let Some(reason) = failure.as_deref() {
        if summary.final_text.is_empty() {
            return Err(AppError::Upstream(format!("claude turn failed ({reason})")));
        }
    }
    let usage = summary
        .usage
        .as_ref()
        .and_then(|v| serde_json::from_value::<CliUsage>(v.clone()).ok())
        .map(|u| u.to_openai())
        .unwrap_or_default();
    Ok(RawResult {
        text: summary.final_text,
        usage,
        stop_reason: Some("end_turn".to_string()),
    })
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Map a driver turn error to an HTTP-shaped error: timeouts → 504, else 502.
fn map_turn_err(e: crate::driver::DriverError) -> AppError {
    let msg = e.to_string();
    if msg.contains("timed out") {
        AppError::Timeout(msg)
    } else {
        AppError::Upstream(format!("claude turn failed: {msg}"))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Prompt building
// ─────────────────────────────────────────────────────────────────────────────

const REMINDER: &str = "\n\n=== REMINDER ===\nFollow the SYSTEM INSTRUCTIONS exactly. If they \
define an output or tool-calling protocol, obey it literally; never claim a tool ran or a step \
succeeded unless its result actually appears above.";

/// First turn of a session: full system prompt (in the unscored user channel) +
/// the whole conversation.
fn build_full_prompt(system: &str, body: &str) -> String {
    let mut s = String::new();
    if !system.trim().is_empty() {
        s.push_str("=== SYSTEM INSTRUCTIONS (authoritative — these define your behavior; follow exactly) ===\n");
        s.push_str(system.trim());
        s.push_str("\n\n");
    }
    s.push_str("=== CONVERSATION ===\n");
    s.push_str(body);
    if !system.trim().is_empty() {
        s.push_str(REMINDER);
    }
    s
}

/// Continuation turn: only the new user/tool messages (the live session already
/// holds the prior context), with the system prompt re-injected every N turns.
fn build_delta_prompt(delta: &[Message], system: &str, reinject: bool) -> String {
    let mut s = String::new();
    if reinject && !system.trim().is_empty() {
        s.push_str("=== SYSTEM INSTRUCTIONS (reminder — still in force; follow exactly) ===\n");
        s.push_str(system.trim());
        s.push_str("\n\n");
    }
    s.push_str(&render_delta(delta));
    if !system.trim().is_empty() {
        s.push_str(REMINDER);
    }
    s
}

/// Render the delta's *new* inputs (user + tool results). Assistant messages are
/// the client's echo of turns the live session already produced, so they are
/// used only to label tool results, not re-sent.
fn render_delta(delta: &[Message]) -> String {
    let mut call_names: HashMap<String, String> = HashMap::new();
    let mut turns: Vec<String> = Vec::new();
    for msg in delta {
        match msg.role.as_str() {
            "assistant" => {
                if let Some(calls) = &msg.tool_calls {
                    for c in calls {
                        call_names.insert(c.id.clone(), c.function.name.clone());
                    }
                }
                // not re-sent
            }
            "tool" | "function" => {
                let label = msg
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_names.get(id))
                    .cloned()
                    .or_else(|| msg.tool_call_id.clone())
                    .unwrap_or_else(|| "tool".to_string());
                turns.push(format!("Tool result [{label}]: {}", msg.text()));
            }
            "system" | "developer" => { /* re-injection handled separately */ }
            _ => turns.push(format!("User: {}", msg.text())),
        }
    }
    if turns.is_empty() {
        // Nothing new but an assistant echo — nudge continuation.
        "Continue.".to_string()
    } else {
        turns.join("\n\n")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hashing
// ─────────────────────────────────────────────────────────────────────────────

fn msg_hash(m: &Message) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.role.hash(&mut h);
    m.text().hash(&mut h);
    if let Some(tc) = &m.tool_calls {
        serde_json::to_string(tc).unwrap_or_default().hash(&mut h);
    }
    if let Some(id) = &m.tool_call_id {
        id.hash(&mut h);
    }
    h.finish()
}

/// `out[k]` = a stable hash of the first `k` messages (FNV-style fold of each
/// message hash). `out[0]` is the empty-prefix seed.
fn cumulative_hashes(messages: &[Message]) -> Vec<u64> {
    let mut out = Vec::with_capacity(messages.len() + 1);
    let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
    out.push(acc);
    for m in messages {
        acc = (acc ^ msg_hash(m)).wrapping_mul(0x100_0000_01b3);
        out.push(acc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::Content;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: Some(Content::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn cumulative_prefix_matches() {
        let a = vec![msg("system", "S"), msg("user", "hi")];
        let b = vec![msg("system", "S"), msg("user", "hi"), msg("assistant", "yo"), msg("user", "again")];
        let ca = cumulative_hashes(&a);
        let cb = cumulative_hashes(&b);
        // a is a prefix of b: the cumulative hash at len(a) must agree.
        assert_eq!(*ca.last().unwrap(), cb[a.len()]);
    }

    #[test]
    fn divergent_prefix_does_not_match() {
        let a = vec![msg("system", "S"), msg("user", "hi")];
        let b = vec![msg("system", "S"), msg("user", "HELLO")];
        let ca = cumulative_hashes(&a);
        let cb = cumulative_hashes(&b);
        assert_ne!(*ca.last().unwrap(), cb[a.len()]);
    }

    #[test]
    fn delta_renders_tool_results_skips_assistant() {
        let mut a = msg("assistant", "");
        a.tool_calls = Some(vec![crate::api::types::ToolCall {
            id: "c1".into(),
            r#type: "function".into(),
            function: crate::api::types::FunctionCall { name: "read".into(), arguments: "{}".into() },
        }]);
        let mut t = msg("tool", "file contents");
        t.tool_call_id = Some("c1".into());
        let body = render_delta(&[a, t]);
        assert!(!body.contains("Assistant"));
        assert!(body.contains("Tool result [read]: file contents"));
    }

    #[test]
    fn delta_only_assistant_yields_continue() {
        let mut a = msg("assistant", "done");
        a.tool_calls = None;
        assert_eq!(render_delta(std::slice::from_ref(&a)), "Continue.");
    }

    #[test]
    fn full_prompt_has_system_and_conversation() {
        let p = build_full_prompt("Be terse.", "User: hi");
        assert!(p.contains("SYSTEM INSTRUCTIONS"));
        assert!(p.contains("Be terse."));
        assert!(p.contains("User: hi"));
        assert!(p.contains("REMINDER"));
    }

    #[test]
    fn delta_prompt_reinjects_only_when_asked() {
        let d = vec![msg("user", "next")];
        let sys = "ZZ_SYSTEM_BODY_ZZ";
        assert!(!build_delta_prompt(&d, sys, false).contains(sys));
        assert!(build_delta_prompt(&d, sys, true).contains(sys));
    }
}
