import test from 'node:test';
import assert from 'node:assert/strict';
import { MIN_QUERY, searchable, searchResult, liveSessionFor, pickHit, moveSel } from '../app/hits.js';

test('a query needs two characters', () => {
  assert.equal(MIN_QUERY, 2);
  assert.equal(searchable(' a '), false);
  assert.equal(searchable('ab'), true);
  assert.equal(searchable('é'), true, 'two bytes, as the daemon counts');
  assert.equal(searchable('a'), false);
  assert.equal(searchable('字'), true, 'one CJK character is three bytes');
});

test('hits are shown newest first with the failures and the cut', () => {
  const r = searchResult({ hits: [{ conversation: 'a', modified: 1 }, { conversation: 'b', modified: 3 }, { conversation: 'c', modified: 2 }],
    failed: [{ host: 'z6', error: 'timed out' }], truncated: true });
  assert.deepEqual(r.hits.map((h) => h.conversation), ['b', 'c', 'a']);
  assert.deepEqual(r.failed, [{ host: 'z6', error: 'timed out' }]);
  assert.equal(r.truncated, true);
  assert.deepEqual(searchResult(undefined), { hits: [], failed: [], truncated: false });
});

test('picking a hit focuses its live session, else resumes it on its host', () => {
  const sessions = {
    s1: { id: 's1', host: 'local', claude_session_id: 'conv', dormant: false, state: 'idle' },
    s2: { id: 's2', host: 'z6', claude_session_id: 'conv2', dormant: true, state: 'idle' },
    s3: { id: 's3', host: 'z6', claude_session_id: 'conv3', dormant: false, state: 'exited' },
  };
  assert.deepEqual(pickHit(sessions, { host: 'local', conversation: 'conv', cwd: '/a' }), { focus: 's1' });
  assert.equal(liveSessionFor(sessions, { host: 'z6', conversation: 'conv' }), null, 'another host');
  for (const conversation of ['conv2', 'conv3']) {
    assert.deepEqual(pickHit(sessions, { host: 'z6', conversation, cwd: '/srv' }), { resume: { host: 'z6', cwd: '/srv', conversation } });
  }
});

test('the highlight wraps over the hits', () => {
  assert.equal(moveSel(0, 1, 3), 1);
  assert.equal(moveSel(2, 1, 3), 0);
  assert.equal(moveSel(0, -1, 3), 2);
  assert.equal(moveSel(0, 1, 0), 0);
});
