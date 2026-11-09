import test from 'node:test';
import assert from 'node:assert/strict';
import {
  uuidToBytes, bytesToUuid, encodeInput, decodeOutput, backoffDelay, versionAction,
  Connection, KIND_INPUT, KIND_OUTPUT, MAX_INPUT, PROTO_VERSION,
} from '../app/proto.js';

const ID = '6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b';

class FakeWS {
  static instances = [];
  constructor(url) { this.url = url; this.sent = []; this.readyState = 0; FakeWS.instances.push(this); }
  send(d) { this.sent.push(d); }
  close() { this.readyState = 3; this.onclose?.({ code: 1000 }); }
  open() { this.readyState = 1; this.onopen?.({}); }
  text(o) { this.onmessage?.({ data: JSON.stringify(o) }); }
  binary(u8) { this.onmessage?.({ data: u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength) }); }
  drop() { this.readyState = 3; this.onclose?.({ code: 1006 }); }
}

function timers() {
  const q = [];
  let id = 0;
  return {
    q,
    setTimeout: (fn, ms) => { const e = { id: ++id, ms, fn: () => { q.splice(q.indexOf(e), 1); fn(); } }; q.push(e); return id; },
    clearTimeout: (i) => { const k = q.findIndex((t) => t.id === i); if (k >= 0) q.splice(k, 1); },
  };
}

function make(extra = {}) {
  FakeWS.instances = [];
  const t = timers();
  const log = [];
  const c = new Connection({
    url: 'ws://x/ws', WebSocketImpl: FakeWS, setTimeout: t.setTimeout, clearTimeout: t.clearTimeout,
    rand: () => 0.5,
    onHello: (h) => log.push(['hello', h.v]),
    onSnapshot: (s) => log.push(['snapshot', s.order]),
    onEvent: (e) => log.push(['ev', e.ev]),
    onOutput: (sid, b) => log.push(['out', sid, b.length]),
    onStatus: (s) => log.push(['status', s]),
    ...extra,
  });
  return { c, t, log };
}

test('uuid round-trips through 16 bytes', () => {
  const b = uuidToBytes(ID);
  assert.equal(b.length, 16);
  assert.equal(b[0], 0x6f);
  assert.equal(b[15], 0x4b);
  assert.equal(bytesToUuid(b), ID);
  assert.equal(bytesToUuid(uuidToBytes(ID.toUpperCase())), ID);
});

test('encodeInput chunks at 64 KiB with kind 0x02 and the id', () => {
  const frames = encodeInput(ID, new Uint8Array(150000).fill(65));
  assert.equal(frames.length, 3);
  assert.deepEqual(frames.map((f) => f.length - 17), [65536, 65536, 18928]);
  for (const f of frames) {
    assert.equal(f[0], KIND_INPUT);
    assert.equal(bytesToUuid(f.subarray(1, 17)), ID);
  }
  assert.equal(MAX_INPUT, 65536);
  const s = encodeInput(ID, 'héllo');
  assert.equal(s.length, 1);
  assert.equal(new TextDecoder().decode(s[0].subarray(17)), 'héllo');
  assert.deepEqual(encodeInput(ID, ''), []);
});

test('decodeOutput reads kind 0x01 only', () => {
  const f = new Uint8Array(20);
  f[0] = KIND_OUTPUT; f.set(uuidToBytes(ID), 1); f.set([1, 2, 3], 17);
  const d = decodeOutput(f.buffer);
  assert.equal(d.sid, ID);
  assert.deepEqual([...d.bytes], [1, 2, 3]);
  f[0] = KIND_INPUT;
  assert.equal(decodeOutput(f.buffer), null);
  assert.equal(decodeOutput(new Uint8Array(16).fill(1).buffer), null);
});

test('backoff doubles from 1 s to 30 s within ±20 %', () => {
  const bases = [1000, 2000, 4000, 8000, 16000, 30000, 30000];
  bases.forEach((b, a) => {
    assert.equal(Math.round(backoffDelay(a, () => 0)), Math.round(b * 0.8));
    assert.equal(Math.round(backoffDelay(a, () => 1)), Math.round(b * 1.2));
    assert.equal(Math.round(backoffDelay(a, () => 0.5)), b);
  });
});

test('request resolves on ok and rejects with the error string', async () => {
  const { c } = make();
  c.start();
  const ws = FakeWS.instances[0];
  ws.open();
  assert.equal(c.status, 'open');
  const p = c.request('session.kill', { sid: ID });
  const sent = JSON.parse(ws.sent[0]);
  assert.deepEqual(sent, { t: 'req', id: 1, op: 'session.kill', sid: ID });
  ws.text({ t: 'res', id: 1, ok: true, data: { x: 1 } });
  assert.deepEqual(await p, { x: 1 });
  const q = c.request('session.kill', { sid: ID });
  ws.text({ t: 'res', id: 2, ok: false, error: 'stale' });
  await assert.rejects(q, new Error('stale'));
  c.stop();
});

test('request rejects with timeout and when not open', async () => {
  const { c, t } = make();
  await assert.rejects(c.request('hosts.list'), new Error('disconnected'));
  c.start();
  FakeWS.instances[0].open();
  const p = c.request('hosts.list', {}, { timeoutMs: 5 });
  const timer = t.q.find((x) => x.ms === 5);
  timer.fn();
  await assert.rejects(p, new Error('timeout'));
  c.stop();
});

test('pending requests reject with "disconnected" and the client reconnects after backoff', async () => {
  const { c, t, log } = make();
  c.start();
  const ws = FakeWS.instances[0];
  ws.open();
  const p = c.request('hosts.list');
  ws.drop();
  await assert.rejects(p, new Error('disconnected'));
  assert.equal(c.status, 'closed');
  const retry = t.q.find((x) => x.ms === 1000);
  assert.ok(retry, 'reconnect scheduled at 1 s (rand 0.5)');
  retry.fn();
  assert.equal(FakeWS.instances.length, 2);
  assert.equal(c.status, 'connecting');
  FakeWS.instances[1].drop();
  assert.ok(t.q.find((x) => x.ms === 2000), 'second attempt doubles');
  t.q.find((x) => x.ms === 2000).fn();
  FakeWS.instances[2].open();
  FakeWS.instances[2].text({ t: 'hello', v: 2 });
  FakeWS.instances[2].drop();
  assert.ok(t.q.find((x) => x.ms === 1000), 'hello resets the attempt counter');
  assert.ok(log.some((e) => e[0] === 'status' && e[1] === 'closed'));
  c.stop();
  assert.equal(t.q.length, 0);
});

test('versionAction reloads once then shows the message', () => {
  assert.equal(PROTO_VERSION, 2);
  assert.equal(versionAction(2, false), 'ok');
  assert.equal(versionAction(2, true), 'ok');
  // a page and a server of different versions: the page reloads once (a version-1 tab open
  // when the server moves to version 2 runs this same rule and comes back as a version-2 page)
  assert.equal(versionAction(1, false), 'reload');
  assert.equal(versionAction(1, true), 'message');
  assert.equal(versionAction(3, false), 'reload');
  assert.equal(versionAction(undefined, false), 'reload');
});

test('hello then snapshot reach their callbacks in order', () => {
  const { c, log } = make();
  c.start();
  const ws = FakeWS.instances[0];
  ws.open();
  ws.text({ t: 'hello', v: 2, version: '0.9.0', features: [] });
  ws.text({ t: 'snapshot', order: ['a'] });
  ws.text({ t: 'ev', ev: 'notice' });
  const f = new Uint8Array(19); f[0] = 1; f.set(uuidToBytes(ID), 1);
  ws.binary(f);
  const seq = log.filter((e) => e[0] !== 'status').map((e) => e[0]);
  assert.deepEqual(seq, ['hello', 'snapshot', 'ev', 'out']);
  c.sendInput(ID, 'ls\r');
  assert.ok(ws.sent.at(-1) instanceof Uint8Array);
  c.stop();
});
