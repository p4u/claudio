// Signals around model.js, fed by the protocol connection.
import { signal, computed } from '@preact/signals';
import { Connection, versionAction } from './proto.js';
import { emptyModel, applySnapshot, applyEvent } from './model.js';
import { toast } from './ui/toasts.js';

export const model = signal(emptyModel());
export const conn = signal('connecting');
export const mode = signal('app'); // 'terminal' | 'app'
export const serverInfo = signal({ version: null, features: [] });
export const banner = signal(null); // sticky page-level message (version mismatch)
export const fresh = signal({}); // sid -> true for ~2 s after it started needing attention

let connection = null;
const outputs = new Map();
const listeners = new Set();
const RELOADED = 'claudio.reloaded';

export function onOutput(sid, fn) {
  if (!outputs.has(sid)) outputs.set(sid, new Set());
  outputs.get(sid).add(fn);
  return () => {
    const set = outputs.get(sid);
    set?.delete(fn);
    if (set?.size === 0) outputs.delete(sid);
  };
}

// Every server event, after the model has been updated (notice, extensions_changed, ...).
export function onServerEvent(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

const asking = (s) => s && (s.state === 'needs_approval' || s.state === 'needs_input');

function markFresh(sid) {
  fresh.value = { ...fresh.value, [sid]: true };
  setTimeout(() => {
    const { [sid]: _, ...rest } = fresh.value;
    fresh.value = rest;
  }, 2200);
}

function handleEvent(ev) {
  const before = ev.ev === 'session' ? model.value.sessions[ev.session.id] : null;
  model.value = applyEvent(model.value, ev);
  if (ev.ev === 'session' && asking(ev.session) && !(asking(before) && before.state === ev.session.state)) markFresh(ev.session.id);
  if (ev.ev === 'notice') toast({ text: ev.text, level: ev.level === 'warn' || ev.level === 'error' ? ev.level : 'info' });
  for (const fn of listeners) fn(ev);
}

function onHello(h) {
  const reloaded = sessionStorage.getItem(RELOADED) === '1';
  const act = versionAction(h.v, reloaded);
  if (act === 'ok') {
    sessionStorage.removeItem(RELOADED);
    banner.value = null;
    serverInfo.value = { version: h.version ?? null, features: h.features ?? [] };
  } else if (act === 'reload') {
    sessionStorage.setItem(RELOADED, '1');
    location.reload();
  } else {
    banner.value = 'The server was updated. Reload the page.';
    connection?.stop();
  }
}

export function connect() {
  if (connection) return connection;
  const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`;
  connection = new Connection({
    url,
    // A deadline: an answer that never comes must not hold the reconnect back.
    me: () => fetch('/api/me', { credentials: 'same-origin', signal: AbortSignal.timeout(10_000) }).then((r) => r.status),
    reload: () => location.reload(),
    onHello,
    onSnapshot: (snap) => {
      model.value = applySnapshot(model.value, snap);
      for (const fn of listeners) fn({ ev: 'snapshot' });
    },
    onEvent: handleEvent,
    onOutput: (sid, bytes) => { for (const fn of outputs.get(sid) ?? []) fn(bytes); },
    onStatus: (s) => { conn.value = s; },
  });
  connection.start();
  return connection;
}

// Resolves with the reply's data. A failure shows an error toast, then rejects as well.
export function req(op, args) {
  if (!connection) return Promise.reject(new Error('disconnected'));
  return connection.request(op, args).catch((e) => {
    toast({ text: `${op}: ${e.message}`, level: 'error' });
    throw e;
  });
}

// Like req, but a failure does not toast: the caller decides what to tell the person.
export function reqQuiet(op, args) {
  if (!connection) return Promise.reject(new Error('disconnected'));
  return connection.request(op, args);
}

export function sendInput(sid, data) {
  connection?.sendInput(sid, data);
}

export function setMode(m) {
  mode.value = m;
}

// The session the rail points at (a session id or null).
export const selected = signal(null);

// The session shown in the focused pane (a session id or null).
export const focused = computed(() => {
  const lay = model.value.layout;
  return lay.panes[lay.focus] ?? null;
});
