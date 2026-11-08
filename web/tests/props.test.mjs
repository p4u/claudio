import test from 'node:test';
import assert from 'node:assert/strict';
import { mergeHandlers } from '../app/props.js';

test('same-named handlers run in order with the same arguments', () => {
  const log = [];
  const m = mergeHandlers({ onDragStart: (e) => log.push(['a', e]) }, { onDragStart: (e) => log.push(['b', e]) });
  m.onDragStart('ev');
  assert.deepEqual(log, [['a', 'ev'], ['b', 'ev']]);
});

test('three objects chain, other keys take the last value', () => {
  const log = [];
  const m = mergeHandlers({ onFocus: () => log.push(1), draggable: 'true' }, { onFocus: () => log.push(2) }, { onFocus: () => log.push(3), draggable: 'false', onBlur: () => log.push(4) });
  m.onFocus(); m.onBlur();
  assert.deepEqual(log, [1, 2, 3, 4]);
  assert.equal(m.draggable, 'false');
});

test('the inputs are not mutated and undefined is skipped', () => {
  const a = { onClick() {} };
  const m = mergeHandlers(a, undefined, { onClick() {} });
  assert.notEqual(m.onClick, a.onClick);
  assert.equal(Object.keys(a).length, 1);
});
