import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyLayout, setGrid, dropOnPane, resizePolicy, shouldSendResize, moveBefore, placeBeside } from '../app/panes.js';

test('grid 1/2/4 keeps panes in order and clamps focus', () => {
  assert.deepEqual(emptyLayout(), { grid: 1, panes: [null], focus: 0 });
  let l = { grid: 4, panes: ['a', 'b', 'c', 'd'], focus: 3 };
  l = setGrid(l, 2);
  assert.deepEqual(l, { grid: 2, panes: ['a', 'b'], focus: 1 });
  l = setGrid(l, 1);
  assert.deepEqual(l, { grid: 1, panes: ['a'], focus: 0 });
  l = setGrid(l, 4);
  assert.deepEqual(l, { grid: 4, panes: ['a', null, null, null], focus: 0 });
  assert.deepEqual(setGrid(l, 3), l);
  const before = { grid: 2, panes: ['a', 'b'], focus: 0 };
  setGrid(before, 4);
  assert.deepEqual(before, { grid: 2, panes: ['a', 'b'], focus: 0 });
});

test('dropping a session on a pane moves it there', () => {
  const l = { grid: 2, panes: ['a', 'b'], focus: 0 };
  assert.deepEqual(dropOnPane(l, 1, 'a').panes, [null, 'a']);
  assert.deepEqual(dropOnPane(l, 1, 'c').panes, ['a', 'c']);
  assert.deepEqual(dropOnPane(l, 0, 'a').panes, ['a', 'b']);
  assert.equal(dropOnPane(l, 1, 'c').focus, 1);
  assert.deepEqual(dropOnPane(l, 5, 'c'), l);
  assert.deepEqual(l.panes, ['a', 'b']);
});

test('only the focused pane\'s own fit sends a resize, never an attached echo, and not the same size twice', () => {
  assert.deepEqual(resizePolicy(), { debounceMs: 250, sendOn: 'own-fit-of-focused-pane', never: 'attached' });
  const size = { rows: 30, cols: 100 };
  const base = { focused: true, cause: 'fit', size, lastSent: { rows: 24, cols: 80 } };
  assert.equal(shouldSendResize(base), true);
  assert.equal(shouldSendResize({ ...base, lastSent: null }), true);
  assert.equal(shouldSendResize({ ...base, focused: false }), false);
  assert.equal(shouldSendResize({ ...base, cause: 'attached' }), false);
  assert.equal(shouldSendResize({ ...base, lastSent: { rows: 30, cols: 100 } }), false);
  assert.equal(shouldSendResize({ ...base, size: null }), false);
  assert.equal(shouldSendResize({ ...base, size: { rows: 0, cols: 10 } }), false);
});

test('reordering moves a session before another or to the end', () => {
  assert.deepEqual(moveBefore(['a', 'b', 'c', 'd'], 'd', 'b'), ['a', 'd', 'b', 'c']);
  assert.deepEqual(moveBefore(['a', 'b', 'c'], 'a', null), ['b', 'c', 'a']);
  assert.deepEqual(moveBefore(['a', 'b', 'c'], 'a', 'a'), ['a', 'b', 'c']);
  assert.deepEqual(moveBefore(['a', 'b'], 'x', 'a'), ['a', 'b']);
});

test('a terminal tab opens beside its session', () => {
  assert.deepEqual(placeBeside({ grid: 1, panes: ['a'], focus: 0 }, 'a', 't'), { grid: 2, panes: ['a', 't'], focus: 0 });
  assert.deepEqual(placeBeside({ grid: 2, panes: ['a', null], focus: 0 }, 'a', 't').panes, ['a', 't']);
  assert.deepEqual(placeBeside({ grid: 2, panes: ['a', 'b'], focus: 0 }, 'a', 't').panes, ['a', 't']);
  assert.deepEqual(placeBeside({ grid: 4, panes: ['a', 'b', 'c', 'd'], focus: 3 }, 'd', 't').panes, ['t', 'b', 'c', 'd']);
});
