import test from 'node:test';
import assert from 'node:assert/strict';
import { rank, attentionQueue, cardActions, approveArgs, JUST_IDLE_MS, PERMISSION_KINDS } from '../app/queue.js';

const NOW = 1_000_000_000_000; // ms
const sec = (agoMs) => (NOW - agoMs) / 1000;
const S = (id, o = {}) => ({ id, host: 'local', kind: 'claude', state: 'idle', state_since: sec(1000), attention: null, ...o });
const att = (kind, seq = 1) => ({ seq, kind, message: 'm', tool: null, detail: null });
const H = (features = ['attention']) => ({ name: 'local', features });

test('queue orders approval > input > error > just-idle, oldest first', () => {
  const list = [
    S('idle', { state: 'idle', state_since: sec(60_000) }),
    S('err', { state: 'error', state_since: sec(5_000) }),
    S('inp-new', { state: 'needs_input', state_since: sec(10_000) }),
    S('inp-old', { state: 'needs_input', state_since: sec(90_000) }),
    S('appr-new', { state: 'needs_approval', state_since: sec(1_000) }),
    S('appr-old', { state: 'needs_approval', state_since: sec(50_000) }),
    S('work', { state: 'working' }),
  ];
  assert.deepEqual(attentionQueue(list, NOW).map((s) => s.id), ['appr-old', 'appr-new', 'inp-old', 'inp-new', 'err', 'idle']);
  assert.deepEqual([rank(list[4], NOW), rank(list[2], NOW), rank(list[1], NOW), rank(list[0], NOW), rank(list[6], NOW)], [0, 1, 2, 3, null]);
});

test('idle older than 10 minutes and shells are left out', () => {
  assert.equal(JUST_IDLE_MS, 600000);
  assert.equal(rank(S('a', { state_since: sec(JUST_IDLE_MS + 1000) }), NOW), null);
  assert.equal(rank(S('b', { state_since: sec(JUST_IDLE_MS - 1000) }), NOW), 3);
  assert.equal(rank(S('c', { kind: 'shell', state: 'needs_approval' }), NOW), null);
  assert.equal(rank(S('d', { state: 'exited' }), NOW), null);
  assert.deepEqual(attentionQueue([S('x', { kind: 'shell', state: 'needs_input' }), S('y', { state: 'idle', state_since: sec(JUST_IDLE_MS * 2) })], NOW), []);
});

test('permission prompts get Enter/Esc; elicitation and input get Open only', () => {
  assert.deepEqual(PERMISSION_KINDS, ['permission_prompt', 'worker_permission_prompt']);
  for (const kind of PERMISSION_KINDS) {
    assert.deepEqual(cardActions(S('p', { state: 'needs_approval', attention: att(kind) }), H()), { approve: true, deny: true, open: true, warning: null });
  }
  const openOnly = { approve: false, deny: false, open: true, warning: null };
  assert.deepEqual(cardActions(S('e', { state: 'needs_approval', attention: att('elicitation_dialog') }), H()), openOnly);
  assert.deepEqual(cardActions(S('e2', { state: 'needs_approval', attention: att('elicitation_url_dialog') }), H()), openOnly);
  assert.deepEqual(cardActions(S('i', { state: 'needs_input', attention: att('idle_prompt') }), H()), openOnly);
  assert.deepEqual(cardActions(S('x', { state: 'error' }), H()), openOnly);
  assert.deepEqual(cardActions(S('z'), H()), openOnly);
  // a stale permission kind on a session that no longer asks must not offer approve
  assert.equal(cardActions(S('w', { state: 'working', attention: att('permission_prompt') }), H()).approve, false);
});

test('a host without the attention feature gets approve with seq 0 and the warning', () => {
  const s = S('old', { state: 'needs_approval', attention: null });
  const h = H([]);
  const a = cardActions(s, h);
  assert.equal(a.approve, true);
  assert.equal(a.deny, true);
  assert.equal(a.warning, 'permission or elicitation: check the terminal');
  assert.deepEqual(approveArgs(s, h, 'approve'), { sid: 'old', decision: 'approve', attention_seq: 0 });
  assert.deepEqual(approveArgs(s, h, 'deny'), { sid: 'old', decision: 'deny', attention_seq: 0 });
  // with the feature: the attention's seq
  const p = S('new', { state: 'needs_approval', attention: att('permission_prompt', 7) });
  assert.deepEqual(approveArgs(p, H(), 'approve'), { sid: 'new', decision: 'approve', attention_seq: 7 });
  // with the feature but no attention yet: Open only, no approve
  assert.equal(cardActions(S('n', { state: 'needs_approval', attention: null }), H()).approve, false);
  // unknown host behaves like no feature
  assert.equal(cardActions(s, undefined).approve, true);
});
