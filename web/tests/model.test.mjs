import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyModel, applySnapshot, applyEvent, orderedSessions, groupSessions, railGroups } from '../app/model.js';

const S = (id, o = {}) => ({ id, host: 'local', kind: 'claude', cwd: '/w/a', label: id, state: 'idle', created_at: 1, ...o });
const H = (name) => ({ name, connected: true });

test('snapshot replaces hosts, sessions, order and layout', () => {
  let m = emptyModel();
  m = applySnapshot(m, { hosts: [H('local'), H('z6')], sessions: [S('a'), S('b')], order: ['b', 'a'],
    layout: { grid: 2, panes: ['a', null], focus: 1 }, profiles: { names: ['work'], default: 'work' } });
  assert.deepEqual(Object.keys(m.hosts), ['local', 'z6']);
  assert.deepEqual(Object.keys(m.sessions), ['a', 'b']);
  assert.deepEqual(m.order, ['b', 'a']);
  assert.equal(m.layout.grid, 2);
  assert.deepEqual(m.profiles, { names: ['work'], default: 'work' });
  m = applySnapshot(m, { hosts: [H('local')], sessions: [S('c')], order: [], layout: { grid: 1, panes: [null], focus: 0 } });
  assert.deepEqual(Object.keys(m.sessions), ['c']);
  assert.deepEqual(Object.keys(m.hosts), ['local']);
  assert.deepEqual(m.profiles, { names: [], default: null });
});

test('session upserts and session_removed deletes', () => {
  let m = applySnapshot(emptyModel(), { hosts: [], sessions: [S('a')], order: ['a'] });
  const before = m;
  m = applyEvent(m, { ev: 'session', session: S('a', { state: 'working' }) });
  assert.equal(m.sessions.a.state, 'working');
  assert.equal(before.sessions.a.state, 'idle', 'reducers do not mutate');
  m = applyEvent(m, { ev: 'session', session: S('b') });
  assert.equal(Object.keys(m.sessions).length, 2);
  m = applyEvent(m, { ev: 'session_removed', sid: 'a' });
  assert.deepEqual(Object.keys(m.sessions), ['b']);
  assert.deepEqual(m.order, []);
});

test('host_removed drops the host', () => {
  let m = applySnapshot(emptyModel(), { hosts: [H('local'), H('z6')], sessions: [], order: [] });
  m = applyEvent(m, { ev: 'host', host: { ...H('z6'), connected: false } });
  assert.equal(m.hosts.z6.connected, false);
  m = applyEvent(m, { ev: 'host_stats', host: 'local', cpu: 12, mem_used: 1, mem_total: 2 });
  assert.equal(m.stats.local.cpu, 12);
  m = applyEvent(m, { ev: 'host_removed', name: 'z6' });
  assert.deepEqual(Object.keys(m.hosts), ['local']);
});

test('lens keeps the higher seq and attached resets it', () => {
  let m = applySnapshot(emptyModel(), { hosts: [], sessions: [S('a')], order: [] });
  m = applyEvent(m, { ev: 'lens', sid: 'a', seq: 5, items: [{ tag: 'latex', text: 'x' }] });
  m = applyEvent(m, { ev: 'latex', sid: 'a', seq: 5, formulas: ['x'] });
  m = applyEvent(m, { ev: 'lens', sid: 'a', seq: 3, items: [] });
  assert.equal(m.lens.a.seq, 5);
  assert.equal(m.lens.a.items.length, 1);
  assert.deepEqual(m.lens.a.formulas, ['x']);
  m = applyEvent(m, { ev: 'attached', sid: 'a', rows: 24, cols: 80 });
  m = applyEvent(m, { ev: 'lens', sid: 'a', seq: 3, items: [{ tag: 'k', text: 'y' }] });
  assert.equal(m.lens.a.seq, 3);
  assert.equal(m.lens.a.items[0].text, 'y');
  m = applyEvent(m, { ev: 'session', session: S('a', { state: 'starting' }) });
  m = applyEvent(m, { ev: 'lens', sid: 'a', seq: 1, items: [] });
  assert.equal(m.lens.a.seq, 1);
});

test('other events fill claude, plugins, order, layout and profiles', () => {
  let m = emptyModel();
  m = applyEvent(m, { ev: 'claude', host: 'local', phase: 'offer_update', have: '1', want: '2' });
  assert.equal(m.claude.local.phase, 'offer_update');
  m = applyEvent(m, { ev: 'plugin_status', host: 'local', name: 'p', state: 'ready' });
  assert.equal(m.plugins['local/p'].state, 'ready');
  m = applyEvent(m, { ev: 'order', sids: ['x'] });
  assert.deepEqual(m.order, ['x']);
  m = applyEvent(m, { ev: 'layout', layout: { grid: 4, panes: [null, null, null, null], focus: 0 } });
  assert.equal(m.layout.grid, 4);
  m = applyEvent(m, { ev: 'profiles', names: ['a'], default: null });
  assert.deepEqual(m.profiles.names, ['a']);
  assert.equal(applyEvent(m, { ev: 'nothing-known' }), m);
});

test('orderedSessions follows order then created_at', () => {
  const m = applySnapshot(emptyModel(), { hosts: [], order: ['c', 'gone', 'a'],
    sessions: [S('a', { created_at: 5 }), S('b', { created_at: 9 }), S('c', { created_at: 7 }), S('d', { created_at: 2 })] });
  assert.deepEqual(orderedSessions(m).map((s) => s.id), ['c', 'a', 'd', 'b']);
});

test('groupSessions puts local first and groups by cwd', () => {
  const g = groupSessions([
    S('1', { host: 'z6', cwd: '/srv/app' }),
    S('2', { host: 'local', cwd: '/w/a' }),
    S('3', { host: 'local', cwd: '/w/b' }),
    S('4', { host: 'local', cwd: '/w/a', kind: 'shell' }),
  ]);
  assert.deepEqual(g.map((x) => x.host), ['local', 'z6']);
  assert.deepEqual(g[0].groups.map((x) => x.cwd), ['/w/a', '/w/b']);
  assert.deepEqual(g[0].groups[0].sessions.map((s) => s.id), ['2', '4']);
  assert.equal(g[0].groups[0].label, 'a');
  assert.equal(g[1].groups[0].label, 'app');
});

test('railGroups lists a repo flat unless it holds two sessions on that host', () => {
  const groups = groupSessions([S('a', { cwd: '/w/api' }), S('b', { cwd: '/w/docs' }), S('c', { cwd: '/w/api' }), S('d', { cwd: '/w/parser' })])[0].groups;
  const r = railGroups(groups);
  assert.deepEqual(r.singles.map((s) => s.id), ['b', 'd']);
  assert.deepEqual(r.groups.map((g) => [g.label, g.sessions.map((s) => s.id)]), [['api', ['a', 'c']]]);
  assert.deepEqual(railGroups([]), { singles: [], groups: [] });
});
