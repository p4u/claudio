import test from 'node:test';
import assert from 'node:assert/strict';
import { makeReads } from '../app/reads.js';

test('a read with no write meanwhile is fresh', () => {
  const r = makeReads();
  assert.equal(r.fresh(r.begin()), true);
});

test('a read started before a toggle never puts the old state back', () => {
  // The extensions screen: an event's reload is in flight when the member toggles; its
  // answer (computed before the toggle) arrives after the optimistic update.
  const r = makeReads();
  const before = r.begin();
  r.write();
  assert.equal(r.fresh(before), false, 'answered while the toggle is in flight');
  r.done();
  assert.equal(r.fresh(before), false, 'answered after the toggle ended');
  // The read the toggle's end starts is fresh.
  assert.equal(r.fresh(r.begin()), true);
});

test('a read started while a toggle is in flight is stale, even once it ended', () => {
  const r = makeReads();
  r.write();
  const during = r.begin();
  r.done();
  assert.equal(r.fresh(during), false);
});

test('a read is fresh only once every overlapping toggle ended', () => {
  const r = makeReads();
  r.write();
  r.write();
  r.done();
  const between = r.begin();
  assert.equal(r.fresh(between), false);
  r.done();
  const after = r.begin();
  assert.equal(r.fresh(after), true);
  // An extra done never makes a stale read fresh.
  r.done();
  r.write();
  assert.equal(r.fresh(after), false);
});
