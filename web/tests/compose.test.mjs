import test from 'node:test';
import assert from 'node:assert/strict';
import { createDrafts, PASTE_MAX, tooLarge } from '../app/compose.js';

test('a draft is kept per session until it is cleared', () => {
  const d = createDrafts();
  assert.equal(d.get('a'), '');
  d.set('a', 'one\ntwo');
  d.set('b', 'other');
  assert.equal(d.get('a'), 'one\ntwo');
  assert.equal(d.get('b'), 'other');
  d.clear('a');
  assert.equal(d.get('a'), '');
  assert.equal(d.get('b'), 'other');
});

test('an empty draft is dropped, not stored', () => {
  const d = createDrafts();
  d.set('a', 'x');
  d.set('a', '');
  assert.equal(d.has('a'), false);
});

test('append puts a snippet after the draft on its own line', () => {
  const d = createDrafts();
  assert.equal(d.append('a', 'first'), 'first');
  assert.equal(d.append('a', 'second'), 'first\nsecond');
  d.set('b', 'ends with newline\n');
  assert.equal(d.append('b', 'next'), 'ends with newline\nnext');
});

test('the size limit counts UTF-8 bytes', () => {
  assert.equal(PASTE_MAX, 64 * 1024);
  assert.equal(tooLarge('a'.repeat(PASTE_MAX)), false);
  assert.equal(tooLarge('a'.repeat(PASTE_MAX + 1)), true);
  assert.equal(tooLarge('é'.repeat(PASTE_MAX / 2 + 1)), true);
});
