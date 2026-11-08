// Compose drafts that survive a reload, and the Recent ring. Pure: the storage and the clock are
// passed in (localStorage in the page, fakes in the node tests); imports nothing.
//
// localStorage['claudio.drafts'] = {[sid]: {text, at, label}}: the unsent draft of each session,
// written ~400 ms after the last keystroke. localStorage['claudio.recent'] = [{id, sid, label,
// text, at}], newest first, at most 30: a snapshot of a draft whenever it is sent, inserted,
// cleared, replaced or its session goes away, so nothing a person wrote is lost.
export const DRAFTS_KEY = 'claudio.drafts';
export const RECENT_KEY = 'claudio.recent';
export const RECENT_MAX = 30;
export const TEXT_MAX = 64 * 1024; // bytes kept of one text
export const TOTAL_MAX = 2 * 1024 * 1024; // bytes of both keys together
export const DEBOUNCE_MS = 400;

const enc = new TextEncoder();
const dec = new TextDecoder();
const bytes = (s) => enc.encode(s).length;

// `text` cut to at most `max` bytes, never in the middle of a character.
export function cutBytes(text, max = TEXT_MAX) {
  const b = enc.encode(text);
  if (b.length <= max) return text;
  let end = max;
  while (end > 0 && (b[end] & 0xc0) === 0x80) end--; // back to a character start
  return dec.decode(b.subarray(0, end));
}

function read(storage, key) {
  try { return JSON.parse(storage.getItem(key)); } catch { return null; }
}

const str = (v, d = '') => (typeof v === 'string' ? v : d);
const num = (v, d) => (typeof v === 'number' && Number.isFinite(v) ? v : d);

export function createDraftStore({
  storage,
  now = () => Date.now(),
  schedule = (fn, ms) => setTimeout(fn, ms),
  cancel = (h) => clearTimeout(h),
} = {}) {
  const drafts = new Map(); // sid -> {text, at, label}
  const dirty = new Set();
  let recent = [];
  let timer = null;
  let seq = 0;

  const raw = read(storage ?? { getItem: () => null }, DRAFTS_KEY);
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    for (const [sid, e] of Object.entries(raw)) {
      if (e && typeof e === 'object' && typeof e.text === 'string' && e.text !== '') {
        drafts.set(sid, { text: cutBytes(e.text), at: num(e.at, 0), label: str(e.label) });
      }
    }
  }
  const rawRecent = read(storage ?? { getItem: () => null }, RECENT_KEY);
  if (Array.isArray(rawRecent)) {
    recent = rawRecent
      .filter((r) => r && typeof r === 'object' && typeof r.text === 'string' && r.text.trim() !== '')
      .map((r) => ({ id: str(r.id) || `r${seq++}`, sid: str(r.sid), label: str(r.label), text: cutBytes(r.text), at: num(r.at, 0) }))
      .slice(0, RECENT_MAX);
  }

  function persist() {
    // the budget: the oldest Recent goes first, then the oldest draft
    for (;;) {
      const d = {};
      for (const [sid, e] of drafts) d[sid] = { text: cutBytes(e.text), at: e.at, label: e.label };
      const ds = JSON.stringify(d);
      const rs = JSON.stringify(recent.map((r) => ({ ...r, text: cutBytes(r.text) })));
      if (bytes(ds) + bytes(rs) <= TOTAL_MAX) {
        try { storage.setItem(DRAFTS_KEY, ds); storage.setItem(RECENT_KEY, rs); } catch { /* full or private mode: the live copy still works */ }
        return;
      }
      if (recent.length) { recent.pop(); continue; }
      let oldest = null;
      for (const [sid, e] of drafts) if (oldest === null || e.at < drafts.get(oldest).at) oldest = sid;
      if (oldest === null) return;
      drafts.delete(oldest);
    }
  }

  function flush() {
    if (timer !== null) { cancel(timer); timer = null; }
    const t = now();
    for (const sid of dirty) { const e = drafts.get(sid); if (e) e.at = t; }
    dirty.clear();
    persist();
  }
  const later = () => {
    if (timer !== null) cancel(timer);
    timer = schedule(() => { timer = null; flush(); }, DEBOUNCE_MS);
  };

  function snapshot(sid, label, text) {
    if (typeof text !== 'string' || text.trim() === '') return;
    recent = recent.filter((r) => r.text !== text);
    recent.unshift({ id: `${now().toString(36)}-${seq++}`, sid, label: label ?? '', text, at: now() });
    if (recent.length > RECENT_MAX) recent.length = RECENT_MAX;
    later();
  }

  return {
    get: (sid) => drafts.get(sid)?.text ?? '',
    has: (sid) => drafts.has(sid),
    // the time the draft was last written to the storage (ms), or null while it is only in memory
    savedAt: (sid) => (drafts.has(sid) && !dirty.has(sid) ? drafts.get(sid).at : null),
    set(sid, text, label = '') {
      if (text === '') { this.clear(sid); return; }
      drafts.set(sid, { text, at: now(), label: label || drafts.get(sid)?.label || '' });
      dirty.add(sid);
      later();
    },
    clear(sid) {
      if (!drafts.delete(sid)) return;
      dirty.delete(sid);
      later();
    },
    // `text` on its own line after the draft; returns the new draft
    append(sid, text, label = '') {
      const cur = drafts.get(sid)?.text ?? '';
      const next = cur + (cur && !cur.endsWith('\n') ? '\n' : '') + text;
      this.set(sid, next, label);
      return next;
    },
    snapshot,
    // the session is gone: its draft becomes a Recent item
    drop(sid) {
      const e = drafts.get(sid);
      if (!e) return;
      snapshot(sid, e.label, e.text);
      this.clear(sid);
    },
    // after a reload: the drafts of sessions that no longer exist become Recent items
    sweep(liveSids) {
      for (const sid of [...drafts.keys()]) if (!liveSids.has(sid)) this.drop(sid);
    },
    recent: () => recent.map((r) => ({ ...r })),
    remove(id) {
      const n = recent.length;
      recent = recent.filter((r) => r.id !== id);
      if (recent.length !== n) later();
    },
    flush,
  };
}
