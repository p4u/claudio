import test from 'node:test';
import assert from 'node:assert/strict';
import { segments, level, ctxFraction, contextWindow } from '../app/gauge.js';

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
  // Changed 2026-11-08: past 200k the window is the 1M one (this used to read 1, i.e. full).
  assert.equal(ctxFraction({ context_tokens: 400000, model: 'opus' }), 0.4);
  assert.equal(ctxFraction({ context_tokens: null, model: 'opus' }), null);
  assert.equal(ctxFraction({ model: 'opus' }), null);
});

test('Opus 5.x runs at 1M without the suffix; past 200k means the 1M window', () => {
  assert.equal(contextWindow('claude-opus-5-5', 0), 1_000_000);
  assert.equal(contextWindow('Claude-Opus-5-1', 10), 1_000_000);
  assert.equal(contextWindow('claude-sonnet-5-5', 0), 200_000);
  assert.equal(contextWindow('claude-sonnet-5-5', 200_000), 200_000);
  assert.equal(contextWindow('claude-sonnet-5-5', 200_001), 1_000_000);
  assert.equal(contextWindow('claude-sonnet-4-5-1m', 0), 1_000_000);
  assert.equal(contextWindow('claude-opus-4-1-20250805', 0), 200_000);
  assert.equal(ctxFraction({ context_tokens: 765_000, model: 'claude-opus-5-5' }), 0.765);
  assert.equal(ctxFraction({ context_tokens: 260_000, model: 'some-proxy-model' }), 0.26);
});
