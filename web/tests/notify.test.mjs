import test from 'node:test';
import assert from 'node:assert/strict';
import { makeNotifier, titleText } from '../app/notify.js';

const S = (id, state, o = {}) => ({ id, label: `lbl-${id}`, kind: 'claude', state, attention: null, ...o });

test('a hidden session needing approval notifies once', () => {
  const n = makeNotifier();
  const s = S('a', 'needs_approval', { attention: { seq: 1, kind: 'permission_prompt', message: 'Use Bash?' } });
  assert.deepEqual(n.check([s], new Set()), [{ sid: 'a', title: 'lbl-a', body: 'Use Bash?' }]);
  assert.deepEqual(n.check([s], new Set()), []);
  const err = S('b', 'error');
  assert.deepEqual(n.check([err], []), [{ sid: 'b', title: 'lbl-b', body: 'error' }]);
  assert.equal(n.check([S('c', 'needs_input')], [])[0].body, 'needs input');
});

test('a second attention in the same state does not notify again', () => {
  const n = makeNotifier();
  const first = S('a', 'needs_approval', { attention: { seq: 1, message: 'one' } });
  const second = S('a', 'needs_approval', { attention: { seq: 2, message: 'two' } });
  assert.equal(n.check([first], []).length, 1);
  assert.equal(n.check([second], []).length, 0);
  // moving between two attention states is a new (session, state)
  assert.equal(n.check([S('a', 'needs_input')], []).length, 1);
});

test('a visible session never notifies', () => {
  const n = makeNotifier();
  const s = S('a', 'needs_approval');
  assert.deepEqual(n.check([s], new Set(['a'])), []);
  assert.deepEqual(n.check([s], ['a']), []);
  // once it is no longer visible and still waiting, it does
  assert.equal(n.check([s], []).length, 1);
});

test('leaving attention re-arms it', () => {
  const n = makeNotifier();
  assert.equal(n.check([S('a', 'needs_approval')], []).length, 1);
  assert.equal(n.check([S('a', 'working')], []).length, 0);
  assert.equal(n.check([S('a', 'needs_approval')], []).length, 1);
  // idle and exited states never notify
  assert.equal(n.check([S('a', 'idle')], []).length, 0);
  // a removed session is forgotten too
  n.check([], []);
  assert.equal(n.check([S('a', 'needs_approval')], []).length, 1);
});

test('titleText counts', () => {
  assert.equal(titleText(0), 'claudio');
  assert.equal(titleText(3), '(3) claudio');
  assert.equal(titleText(-1), 'claudio');
});
