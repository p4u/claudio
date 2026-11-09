import test from 'node:test';
import assert from 'node:assert/strict';
import { sortSegments, barRows, gaugeOf, short, sections, initialOff, panelFor, tone } from '../app/addon-view.js';

test('segments sort by priority, ties keep their order', () => {
  const s = [{ id: 'c', priority: 150 }, { id: 'a', priority: 10 }, { id: 'b', priority: 10 }, { id: 'd' }];
  assert.deepEqual(sortSegments(s).map((x) => x.id), ['a', 'b', 'c', 'd']);
  assert.deepEqual(sortSegments(undefined), []);
});

test('an unknown tone is muted', () => {
  assert.equal(tone('hot'), 'hot');
  assert.equal(tone('<b>'), 'muted');
});

test('bars are a percentage of the largest value', () => {
  assert.deepEqual(barRows([['24h', 50], ['7d', 100], ['30d', 0]]).map((r) => r.pct), [50, 100, 0]);
  assert.deepEqual(barRows([['a', 0]]).map((r) => r.pct), [0]);
  assert.deepEqual(barRows([['a', -5], ['b', 'x']]).map((r) => [r.value, r.pct]), [[-5, 0], [0, 0]]);
});

test('a gauge is clamped and levelled', () => {
  assert.deepEqual(gaugeOf({ used: 30, limit: 100 }), { used: 30, limit: 100, pct: 30, level: 'ok' });
  assert.equal(gaugeOf({ used: 60, limit: 100 }).level, 'warn');
  assert.equal(gaugeOf({ used: 900, limit: 100 }).pct, 100);
  assert.equal(gaugeOf({ used: 900, limit: 100 }).level, 'hot');
  assert.equal(gaugeOf({ used: 5, limit: 0 }).pct, 0);
});

test('short numbers', () => {
  assert.equal(short(5600), '5.6k');
  assert.equal(short(1_250_000), '1.3M');
  assert.equal(short(12), '12');
  assert.equal(short(null), '0');
});

test('checklist sections break where the section changes', () => {
  const items = [{ id: '1', section: 'MCP', on: true }, { id: '2', section: 'MCP', on: false }, { id: '3', section: null, on: true }, { id: '4', section: 'MCP', on: true }];
  assert.deepEqual(sections(items).map((g) => [g.section, g.items.map((i) => i.id)]), [['MCP', ['1', '2']], [null, ['3']], ['MCP', ['4']]]);
  assert.deepEqual([...initialOff(items)], ['2']);
});

test('the panel for a key, else the first', () => {
  const decls = [{ name: 'a', panels: [{ id: 'one', key: null }] }, { name: 'b', panels: [{ id: 'two', key: 'alt+s' }] }];
  assert.deepEqual(panelFor(decls, 'alt+s'), { addon: 'b', panel: decls[1].panels[0] });
  assert.deepEqual(panelFor(decls, 'alt+q'), { addon: 'a', panel: decls[0].panels[0] });
  assert.equal(panelFor([], 'alt+s'), null);
});
