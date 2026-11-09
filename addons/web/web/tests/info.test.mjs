import test from 'node:test';
import assert from 'node:assert/strict';
import { infoRows, placeTip, duration, INFO_DELAY_MS } from '../app/info.js';

const NOW = 2_000_000;
const S = (o = {}) => ({
  id: 'a', label: 'api', host: 'local', kind: 'claude', cwd: '/w/api', state: 'idle', state_since: NOW - 125, created_at: NOW - 7300,
  branch: 'main', git: { staged: 1, unstaged: 2, untracked: 0, conflicts: 0, ahead: 3, behind: 0 }, model: 'opus', context_tokens: 50_000,
  output_tokens: 1200, turns: 4, launch: { known: true, choice: { addon: 'vocdoni', option: 'team', flags: [] } }, attention: null, ...o,
});
const get = (rows, k) => rows.find((r) => r.k === k)?.v;

test('the delay is 800 ms', () => assert.equal(INFO_DELAY_MS, 800));

test('durations read as the two largest units', () => {
  assert.deepEqual([5, 125, 7300, 90000].map(duration), ['5s', '2m', '2h 1m', '1d 1h']);
});

test('infoRows lists the facts of a claude session', () => {
  const r = infoRows(S(), 'stopped', NOW);
  assert.equal(get(r, 'Name'), 'api');
  assert.equal(get(r, 'State'), 'stopped (idle)');
  assert.equal(get(r, 'Branch'), 'main  +1 ~2 ↑3');
  assert.equal(get(r, 'Context'), '50.0k tokens (25%)');
  assert.equal(get(r, 'Output'), '1.2k tokens');
  assert.equal(get(r, 'Turns'), '4');
  assert.equal(get(r, 'Uptime'), '2h 1m');
  assert.equal(get(r, 'In this state'), '2m');
  assert.equal(get(r, 'Launch'), 'team');
  assert.equal(get(r, 'Kind'), 'claude');
});

test('a waiting session shows what it asks', () => {
  const s = S({ state: 'needs_approval', attention: { seq: 1, kind: 'permission_prompt', message: 'needs Bash', tool: 'Bash', detail: 'ls -la' } });
  assert.equal(get(infoRows(s, 'waiting', NOW), 'State'), 'waiting for you: needs Bash · Bash · ls -la');
});

test('a shell has no model, context or launch rows, and missing values are left out', () => {
  const r = infoRows(S({ kind: 'shell', branch: null, git: null }), 'stopped', NOW);
  for (const k of ['Model', 'Context', 'Output', 'Turns', 'Launch', 'Branch']) assert.equal(get(r, k), undefined, k);
  assert.equal(get(r, 'Kind'), 'terminal');
  assert.equal(get(infoRows(S({ launch: { known: false, choice: null } }), 'stopped', NOW), 'Launch'), 'unknown');
  assert.equal(get(infoRows(S({ launch: { known: true, choice: null } }), 'stopped', NOW), 'Launch'), 'direct');
  assert.equal(get(infoRows(S({ launch: { known: true, choice: { addon: 'vocdoni', option: 'team', flags: ['first-party'] } } }), 'stopped', NOW), 'Launch'), 'team (first-party)');
});

test('server text stays text: values are plain strings', () => {
  const r = infoRows(S({ label: '<img src=x onerror=1>' }), 'stopped', NOW);
  assert.equal(get(r, 'Name'), '<img src=x onerror=1>');
  assert.ok(r.every((x) => typeof x.v === 'string'));
});

test('the popover goes beside the item, never over it, always inside the viewport', () => {
  const vp = { w: 1000, h: 700 };
  const size = { w: 300, h: 200 };
  const rect = { left: 10, top: 100, right: 330, bottom: 140 };
  assert.deepEqual(placeTip(rect, size, vp), { left: 338, top: 100 });
  // near the bottom edge: moved up, still beside
  assert.equal(placeTip({ ...rect, top: 650, bottom: 690 }, size, vp).top, 492);
  // no room right: left
  const r2 = { left: 700, top: 100, right: 990, bottom: 140 };
  assert.equal(placeTip(r2, size, vp).left, 392);
  // a wide row: below, or above when there is no room below
  const row = { left: 330, top: 100, right: 990, bottom: 140 };
  assert.deepEqual(placeTip(row, size, vp, 8, true), { left: 330, top: 148 });
  assert.equal(placeTip({ ...row, top: 600, bottom: 640 }, size, vp, 8, true).top, 392);
});
