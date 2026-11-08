// The attention queue's pure rules (spec F1, AC9). Imports nothing.
// `now` is in milliseconds; Session.state_since is Unix seconds.
export const JUST_IDLE_MS = 10 * 60 * 1000;
export const PERMISSION_KINDS = ['permission_prompt', 'worker_permission_prompt'];
export const NO_ATTENTION_WARNING = 'permission or elicitation: check the terminal';

// 0 needs_approval, 1 needs_input, 2 error, 3 idle within JUST_IDLE_MS of state_since; null otherwise.
export function rank(session, now) {
  if (!session || session.kind === 'shell') return null;
  switch (session.state) {
    case 'needs_approval': return 0;
    case 'needs_input': return 1;
    case 'error': return 2;
    case 'idle': return now - session.state_since * 1000 <= JUST_IDLE_MS ? 3 : null;
    default: return null;
  }
}

export function attentionQueue(sessions, now) {
  return [...sessions]
    .map((s) => ({ s, r: rank(s, now) }))
    .filter((x) => x.r !== null)
    .sort((a, b) => (a.r - b.r) || ((a.s.state_since ?? 0) - (b.s.state_since ?? 0)))
    .map((x) => x.s);
}

const hasAttentionFeature = (host) => !!host && Array.isArray(host.features) && host.features.includes('attention');

// What a queue card may offer. Approve/Deny send Enter/Esc into the terminal, which is only
// right for a permission prompt (Enter on an elicitation picks a choice).
export function cardActions(session, host) {
  const none = { approve: false, deny: false, open: true, warning: null };
  if (!session || session.state !== 'needs_approval') return none;
  const a = session.attention;
  if (a && PERMISSION_KINDS.includes(a.kind)) return { approve: true, deny: true, open: true, warning: null };
  if (!a && !hasAttentionFeature(host)) return { approve: true, deny: true, open: true, warning: NO_ATTENTION_WARNING };
  return none;
}

export function approveArgs(session, host, decision) {
  const seq = hasAttentionFeature(host) ? (session.attention?.seq ?? 0) : 0;
  return { sid: session.id, decision, attention_seq: seq };
}
