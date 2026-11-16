// Conversation search hits, pure part, shared by the search dialog (ui/search.js) and the
// wizard's "Find a conversation…" field (ui/wizard-find.js). Imports nothing.

// The shortest query the daemon takes (claude/search.rs refuses shorter ones as "bad query").
export const MIN_QUERY = 2;

// Whether `text` is worth asking every host about.
export const searchable = (text) => [...String(text ?? '').trim()].length >= MIN_QUERY;

// A `search` reply as shown: hits newest first (the server merges them so; kept stable), the
// hosts that failed, and whether a host cut its list.
export function searchResult(r) {
  const hits = [...(r?.hits ?? [])].sort((a, b) => (b.modified ?? 0) - (a.modified ?? 0));
  return { hits, failed: r?.failed ?? [], truncated: !!r?.truncated };
}

// The live session already on a hit's conversation, or null: same host and conversation, not
// dormant, not exited.
export function liveSessionFor(sessions, { host, conversation }) {
  return Object.values(sessions ?? {}).find((s) => s.host === host && s.claude_session_id === conversation
    && !s.dormant && s.state !== 'exited') ?? null;
}

// What picking a hit does: `{ focus: sid }` (its live session) or `{ resume: {host, cwd,
// conversation} }` (start a claude session there resuming it).
export function pickHit(sessions, hit) {
  const live = liveSessionFor(sessions, hit);
  if (live) return { focus: live.id };
  return { resume: { host: hit.host, cwd: hit.cwd, conversation: hit.conversation } };
}

// The highlight after moving `delta` rows over `n` hits (wrapping; 0 with none).
export const moveSel = (sel, delta, n) => (n ? (((sel + delta) % n) + n) % n : 0);
