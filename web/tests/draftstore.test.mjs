import test from 'node:test';
import assert from 'node:assert/strict';
import { createDraftStore, DRAFTS_KEY, RECENT_KEY, RECENT_MAX, TEXT_MAX, TOTAL_MAX, DEBOUNCE_MS } from '../app/draftstore.js';

function fake() {
  const mem = new Map();
  const storage = { getItem: (k) => mem.get(k) ?? null, setItem: (k, v) => { mem.set(k, v); } };
  const timers = [];
  let t = 1000;
  const opts = { storage, now: () => t, schedule: (fn, ms) => { const h = { fn, ms, off: false }; timers.push(h); return h; }, cancel: (h) => { h.off = true; } };
  const fire = () => { for (const h of timers.splice(0)) if (!h.off) h.fn(); };
  return { mem, storage, opts, timers, fire, tick: (ms) => { t += ms; }, at: () => t };
}
const stored = (f, key) => JSON.parse(f.mem.get(key));

test('a draft autosaves after the debounce, once, with its time and label', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'hel', 'Alpha');
  s.set('a', 'hello', 'Alpha');
  assert.equal(f.mem.has(DRAFTS_KEY), false);
  assert.equal(f.timers.filter((h) => !h.off).length, 1);
  assert.equal(f.timers.at(-1).ms, DEBOUNCE_MS);
  f.tick(400);
  f.fire();
  assert.deepEqual(stored(f, DRAFTS_KEY), { a: { text: 'hello', at: 1400, label: 'Alpha' } });
  assert.equal(s.savedAt('a'), 1400);
});

test('a draft comes back after a reload', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'keep me', 'Alpha');
  s.flush();
  const again = createDraftStore(f.opts);
  assert.equal(again.get('a'), 'keep me');
  assert.equal(again.has('a'), true);
  assert.equal(again.savedAt('a'), 1000);
  assert.equal(again.get('zzz'), '');
});

test('an emptied draft is dropped from the storage', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'x', 'A');
  s.flush();
  s.set('a', '', 'A');
  s.flush();
  assert.deepEqual(stored(f, DRAFTS_KEY), {});
});

test('garbage in the storage is ignored', () => {
  const f = fake();
  f.mem.set(DRAFTS_KEY, '{"a":{"text":5},"b":"x","c":{"text":"ok","at":"no","label":3}}');
  f.mem.set(RECENT_KEY, 'not json');
  const s = createDraftStore(f.opts);
  assert.equal(s.get('a'), '');
  assert.equal(s.get('b'), '');
  assert.equal(s.get('c'), 'ok');
  assert.deepEqual(s.recent(), []);
});

test('a text is cut to 64 KiB for storage, never in the middle of a character', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'é'.repeat(TEXT_MAX), 'A'); // 2 bytes each
  s.flush();
  const text = stored(f, DRAFTS_KEY).a.text;
  assert.ok(new TextEncoder().encode(text).length <= TEXT_MAX);
  assert.ok(!text.includes('�'));
  assert.equal(text, 'é'.repeat(TEXT_MAX / 2));
  // the live text keeps everything (the editor shows its own warning)
  assert.equal(s.get('a').length, TEXT_MAX);
});

test('snapshots go to Recent, newest first, a repeat only moves up', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.snapshot('a', 'Alpha', 'one');
  f.tick(5);
  s.snapshot('b', 'Beta', 'two\nlines');
  f.tick(5);
  s.snapshot('a', 'Alpha', 'one');
  const r = s.recent();
  assert.deepEqual(r.map((x) => x.text), ['one', 'two\nlines']);
  assert.equal(r[0].label, 'Alpha');
  assert.equal(r[0].at, 1010);
  assert.equal(r[0].sid, 'a');
  assert.ok(r.every((x) => x.id));
});

test('a blank snapshot is not kept', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.snapshot('a', 'A', '  \n ');
  s.snapshot('a', 'A', '');
  assert.deepEqual(s.recent(), []);
});

test('Recent is a ring of 30: the oldest is dropped', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  assert.equal(RECENT_MAX, 30);
  for (let i = 0; i < 35; i++) { f.tick(1); s.snapshot('a', 'A', `text ${i}`); }
  const r = s.recent();
  assert.equal(r.length, 30);
  assert.equal(r[0].text, 'text 34');
  assert.equal(r[29].text, 'text 5');
});

test('Recent items can be removed, and persist', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.snapshot('a', 'A', 'one');
  f.tick(1);
  s.snapshot('a', 'A', 'two');
  const [two] = s.recent();
  s.remove(two.id);
  s.flush();
  assert.deepEqual(createDraftStore(f.opts).recent().map((x) => x.text), ['one']);
});

test('the total budget drops the oldest Recent first, then the oldest drafts', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  const big = 'x'.repeat(TEXT_MAX);
  for (let i = 0; i < 30; i++) { f.tick(1); s.snapshot('a', 'A', `${i}${big}`.slice(0, TEXT_MAX)); }
  f.tick(1);
  for (const k of ['live', 'live2', 'live3']) { f.tick(1); s.set(k, big, k); }
  s.flush();
  const size = (key) => f.mem.get(key).length;
  assert.ok(size(DRAFTS_KEY) + size(RECENT_KEY) <= TOTAL_MAX, 'within budget');
  const left = stored(f, RECENT_KEY);
  assert.ok(left.length < 30);
  assert.equal(left[0].text.startsWith('29'), true, 'the newest survives');
  assert.equal(stored(f, DRAFTS_KEY).live.text, big, 'the draft survives');
});

test('when drafts alone are over budget the oldest draft goes', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  const big = 'y'.repeat(TEXT_MAX);
  for (let i = 0; i < 40; i++) { f.tick(1); s.set(`s${i}`, big, `S${i}`); }
  s.flush();
  const d = stored(f, DRAFTS_KEY);
  assert.ok(f.mem.get(DRAFTS_KEY).length <= TOTAL_MAX);
  assert.equal(d.s39.text, big);
  assert.equal(d.s0, undefined);
});

test('a storage that throws never breaks the editor', () => {
  const bad = { getItem: () => { throw new Error('no'); }, setItem: () => { throw new Error('full'); } };
  const s = createDraftStore({ storage: bad, schedule: () => 0, cancel: () => {} });
  s.set('a', 'x', 'A');
  s.snapshot('a', 'A', 'x');
  s.flush();
  assert.equal(s.get('a'), 'x');
});

test('append puts a snippet on its own line after the draft', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  assert.equal(s.append('a', 'first', 'A'), 'first');
  assert.equal(s.append('a', 'second', 'A'), 'first\nsecond');
});

test('sweep moves the drafts of vanished sessions to Recent', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'alive', 'A');
  s.set('b', 'gone', 'B');
  s.sweep(new Set(['a']));
  assert.equal(s.has('b'), false);
  assert.equal(s.get('a'), 'alive');
  assert.deepEqual(s.recent().map((x) => [x.label, x.text]), [['B', 'gone']]);
});

test('drop snapshots and forgets a session draft', () => {
  const f = fake();
  const s = createDraftStore(f.opts);
  s.set('a', 'bye', 'A');
  s.drop('a');
  assert.equal(s.has('a'), false);
  assert.equal(s.recent()[0].text, 'bye');
});
