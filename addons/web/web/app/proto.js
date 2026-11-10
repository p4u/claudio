// Browser protocol v2 client (docs/web-protocol.md). Pure apart from the injected
// WebSocket, timers, me() and reload(); imports nothing.
export const PROTO_VERSION = 2;
export const KIND_OUTPUT = 0x01;
export const KIND_INPUT = 0x02;
export const MAX_INPUT = 64 * 1024;

const HEX = '0123456789abcdef';

export function uuidToBytes(id) {
  const hex = String(id).replace(/-/g, '').toLowerCase();
  if (!/^[0-9a-f]{32}$/.test(hex)) throw new Error('bad uuid');
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToUuid(u8) {
  let s = '';
  for (let i = 0; i < 16; i++) {
    s += HEX[u8[i] >> 4] + HEX[u8[i] & 15];
    if (i === 3 || i === 5 || i === 7 || i === 9) s += '-';
  }
  return s;
}

export function encodeInput(id, data) {
  const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
  const idb = uuidToBytes(id);
  const frames = [];
  for (let off = 0; off < bytes.length; off += MAX_INPUT) {
    const chunk = bytes.subarray(off, Math.min(off + MAX_INPUT, bytes.length));
    const f = new Uint8Array(17 + chunk.length);
    f[0] = KIND_INPUT;
    f.set(idb, 1);
    f.set(chunk, 17);
    frames.push(f);
  }
  return frames;
}

export function decodeOutput(buf) {
  const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (u8.length < 17 || u8[0] !== KIND_OUTPUT) return null;
  return { sid: bytesToUuid(u8.subarray(1, 17)), bytes: u8.subarray(17) };
}

export function backoffDelay(attempt, rand = Math.random) {
  const base = Math.min(30000, 1000 * 2 ** Math.min(attempt, 10));
  return base * (0.8 + 0.4 * rand());
}

export function versionAction(v, reloadedBefore) {
  if (v === PROTO_VERSION) return 'ok';
  return reloadedBefore ? 'message' : 'reload';
}

export class Connection {
  constructor({ url, WebSocketImpl = globalThis.WebSocket, setTimeout: st = globalThis.setTimeout,
    clearTimeout: ct = globalThis.clearTimeout, rand = Math.random, me = null, reload = null,
    onHello, onSnapshot, onEvent, onOutput, onStatus } = {}) {
    this.url = url;
    this.WS = WebSocketImpl;
    this.st = (fn, ms) => st(fn, ms);
    this.ct = (id) => ct(id);
    this.rand = rand;
    // me(): the status of GET /api/me; reload(): the page's. Both come from the caller.
    this.me = me;
    this.reload = reload;
    this.gen = 0; // bumped by every open and stop: a late me() answer is then ignored
    this.cb = { onHello, onSnapshot, onEvent, onOutput, onStatus };
    this._status = 'closed';
    this.ws = null;
    this.pending = new Map();
    this.nextId = 1;
    this.attempt = 0;
    this.timer = null;
    this.stopped = true;
  }

  get status() { return this._status; }

  _setStatus(s) {
    if (this._status === s) return;
    this._status = s;
    this.cb.onStatus?.(s);
  }

  start() {
    this.stopped = false;
    this._open();
  }

  stop() {
    this.stopped = true;
    this.gen++;
    if (this.timer != null) { this.ct(this.timer); this.timer = null; }
    const ws = this.ws;
    this.ws = null;
    if (ws) { ws.onclose = null; try { ws.close(); } catch { /* already closed */ } }
    this._rejectAll('disconnected');
    this._setStatus('closed');
  }

  _open() {
    this.timer = null;
    this.gen++;
    this._setStatus('connecting');
    const ws = new this.WS(this.url);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onopen = () => { if (this.ws === ws) this._setStatus('open'); };
    ws.onmessage = (m) => { if (this.ws === ws) this._message(m.data); };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this._rejectAll('disconnected');
      this._setStatus('closed');
      if (!this.stopped) this._retry();
    };
  }

  // A close may be a sign-in that ended: ask me() first, and on an explicit 401 reload (the
  // page then shows its login). Any other status, or a failure, reconnects after the backoff.
  _retry() {
    const gen = this.gen;
    const later = () => {
      if (this.stopped || gen !== this.gen) return;
      const d = backoffDelay(this.attempt++, this.rand);
      this.timer = this.st(() => this._open(), d);
    };
    if (!this.me || !this.reload) { later(); return; }
    Promise.resolve().then(() => this.me()).then((status) => {
      if (status !== 401 || this.stopped || gen !== this.gen) { later(); return; }
      this.stopped = true;
      this.reload();
    }, later);
  }

  _rejectAll(msg) {
    const all = [...this.pending.values()];
    this.pending.clear();
    for (const p of all) { this.ct(p.timer); p.reject(new Error(msg)); }
  }

  _message(data) {
    if (typeof data !== 'string') {
      const o = decodeOutput(data);
      if (o) this.cb.onOutput?.(o.sid, o.bytes);
      return;
    }
    let m;
    try { m = JSON.parse(data); } catch { return; }
    if (!m || typeof m !== 'object') return;
    switch (m.t) {
      case 'hello': this.attempt = 0; this.cb.onHello?.(m); break;
      case 'snapshot': this.cb.onSnapshot?.(m); break;
      case 'ev': this.cb.onEvent?.(m); break;
      case 'res': {
        const p = this.pending.get(m.id);
        if (!p) break;
        this.pending.delete(m.id);
        this.ct(p.timer);
        if (m.ok) p.resolve(m.data);
        else p.reject(new Error(m.error ?? 'error'));
        break;
      }
      default: break;
    }
  }

  request(op, args = {}, { timeoutMs = 30000 } = {}) {
    return new Promise((resolve, reject) => {
      if (!this.ws || this._status !== 'open') { reject(new Error('disconnected')); return; }
      const id = this.nextId++;
      if (this.nextId > 0xffffffff) this.nextId = 1;
      const timer = this.st(() => {
        if (this.pending.delete(id)) reject(new Error('timeout'));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.ws.send(JSON.stringify({ ...args, t: 'req', id, op }));
    });
  }

  sendInput(sid, data) {
    if (!this.ws || this._status !== 'open') return;
    for (const f of encodeInput(sid, data)) this.ws.send(f);
  }
}
