import test from 'node:test';
import assert from 'node:assert/strict';
import { linearScale, niceTicks, barLayout, formatTokens } from '../app/charts.js';

test('a linear scale maps the domain onto the range', () => {
  const s = linearScale([0, 10], [100, 0]);
  assert.equal(s(0), 100);
  assert.equal(s(10), 0);
  assert.equal(s(2.5), 75);
  assert.equal(linearScale([5, 5], [0, 10])(5), 0);
});

test('ticks are round numbers up to the max', () => {
  assert.deepEqual(niceTicks(100, 4), [0, 25, 50, 75, 100]);
  const t = niceTicks(87, 4);
  assert.equal(t[0], 0);
  assert.ok(t[t.length - 1] >= 87);
  assert.ok(t.every((v) => Number.isInteger(v)));
  assert.deepEqual(niceTicks(0), [0, 1]);
  const big = niceTicks(310000, 4);
  assert.ok(big[big.length - 1] >= 310000 && big[big.length - 1] <= 400000);
});

test('bars fill the width with gaps', () => {
  const bars = barLayout([10, 5, 0], 100, 50, 2);
  assert.equal(bars.length, 3);
  const w = (100 - 2 * 2) / 3;
  assert.ok(Math.abs(bars[0].w - w) < 1e-9);
  assert.ok(Math.abs(bars[2].x + bars[2].w - 100) < 1e-9);
  assert.equal(bars[0].h, 50);
  assert.equal(bars[0].y, 0);
  assert.equal(bars[1].h, 25);
  assert.equal(bars[1].y, 25);
  assert.equal(bars[2].h, 0);
  assert.deepEqual(barLayout([], 100, 50), []);
});

test('token counts format compactly', () => {
  assert.equal(formatTokens(950), '950');
  assert.equal(formatTokens(5600), '5.6k');
  assert.equal(formatTokens(2_300_000), '2.3M');
  assert.equal(formatTokens(0), '0');
  assert.equal(formatTokens(null), '–');
  assert.equal(formatTokens(12_000), '12k');
});
