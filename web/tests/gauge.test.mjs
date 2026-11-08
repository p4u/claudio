import test from 'node:test';
import assert from 'node:assert/strict';
import { segments, level, ctxFraction } from '../app/gauge.js';

test('8 segments with 60/85 thresholds', () => {
  assert.equal(segments(0), '░░░░░░░░');
  assert.equal(segments(1), '████████');
  assert.equal(segments(0.5), '████░░░░');
  assert.equal(segments(0.51), '████░░░░');
  assert.equal(segments(2), '████████');
  assert.equal(segments(-1), '░░░░░░░░');
  assert.equal(segments(0.5, 4), '██░░');
  assert.equal(segments(0.5).length, 8);
  assert.equal(level(0), 'ok');
  assert.equal(level(0.59), 'ok');
  assert.equal(level(0.6), 'warn');
  assert.equal(level(0.84), 'warn');
  assert.equal(level(0.85), 'hot');
  assert.equal(level(1), 'hot');
});

test('the context window is 1M for [1m] models', () => {
  assert.equal(ctxFraction({ context_tokens: 100000, model: 'opus' }), 0.5);
  assert.equal(ctxFraction({ context_tokens: 100000, model: 'opus' }, 'claude-opus-4-1[1m]'), 0.1);
  assert.equal(ctxFraction({ context_tokens: 100000, model: 'opus[1m]' }), 0.1);
  assert.equal(ctxFraction({ context_tokens: 100000, model: 'opus[1M]' }), 0.1);
  assert.equal(ctxFraction({ context_tokens: 400000, model: 'opus' }), 1);
  assert.equal(ctxFraction({ context_tokens: null, model: 'opus' }), null);
  assert.equal(ctxFraction({ model: 'opus' }), null);
});
