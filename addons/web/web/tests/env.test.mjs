import test from 'node:test';
import assert from 'node:assert/strict';
import { nameOk, normalizeValue, valueRows, nameNote, scopeArgs } from '../app/env.js';

const catalogue = {
  vars: [{ name: 'ANTHROPIC_DEFAULT_OPUS_MODEL', category: 'model', description: 'Model ID that the opus alias resolves to.' }],
  refused: [{ name: 'PATH', reason: 'the daemon sets it' }],
};

test('names are letters, digits and underscores, not starting with a digit', () => {
  for (const good of ['A', '_', 'https_proxy', 'MAX_THINKING_TOKENS', 'A'.repeat(128)]) assert.ok(nameOk(good), good);
  for (const bad of ['', '1A', 'A-B', 'A B', 'Ä', 'A'.repeat(129)]) assert.ok(!nameOk(bad), bad);
});

test('a value keeps newlines and tabs; CRLF and CR become LF', () => {
  assert.equal(normalizeValue('a\r\nb\rc\nd\te'), 'a\nb\nc\nd\te');
  assert.equal(valueRows('one'), 1);
  assert.equal(valueRows('a\nb\nc'), 3);
  assert.equal(valueRows('\n'.repeat(20)), 8);
});

test('the add row describes a documented name and says why one is refused', () => {
  assert.deepEqual(nameNote(catalogue, ''), {});
  assert.deepEqual(nameNote(catalogue, ' PATH '), { refused: 'the daemon sets it' });
  assert.deepEqual(nameNote(catalogue, '1X'), { bad: true });
  assert.equal(nameNote(catalogue, 'ANTHROPIC_DEFAULT_OPUS_MODEL').known.category, 'model');
  assert.deepEqual(nameNote(catalogue, 'MY_OWN'), {});
});

test('a tab is the global scope, or the focused session', () => {
  assert.deepEqual(scopeArgs('global', 'abc'), { scope: 'global' });
  assert.deepEqual(scopeArgs('session', 'abc'), { scope: 'session', sid: 'abc' });
  assert.deepEqual(scopeArgs('session', null), { scope: 'global' });
});
