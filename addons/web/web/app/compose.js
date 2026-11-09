// Prompt drafts of the Compose editor, one per session, in memory. Pure; imports nothing.
export const PASTE_MAX = 64 * 1024; // the server's limit for one paste
const enc = new TextEncoder();

export const tooLarge = (text) => enc.encode(text).length > PASTE_MAX;

export function createDrafts() {
  const m = new Map();
  return {
    get: (sid) => m.get(sid) ?? '',
    has: (sid) => m.has(sid),
    set(sid, text) { if (text === '') m.delete(sid); else m.set(sid, text); },
    clear: (sid) => { m.delete(sid); },
    // add `text` on its own line after the draft; returns the new draft
    append(sid, text) {
      const cur = m.get(sid) ?? '';
      const next = cur + (cur && !cur.endsWith('\n') ? '\n' : '') + text;
      if (next === '') m.delete(sid); else m.set(sid, next);
      return next;
    },
  };
}
