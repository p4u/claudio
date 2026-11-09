// Notification debounce (the rule of src/tui/notifications.rs). Imports nothing.
const WANTS = { needs_approval: 'needs approval', needs_input: 'needs input', error: 'error' };

export function makeNotifier() {
  const armed = new Map(); // sid -> the state already notified
  return {
    // sessions: array (or object) of Session; visible: Set or array of sids shown in a pane.
    check(sessions, visible) {
      const list = Array.isArray(sessions) ? sessions : Object.values(sessions ?? {});
      const seen = visible instanceof Set ? visible : new Set(visible ?? []);
      const live = new Set();
      const out = [];
      for (const s of list) {
        live.add(s.id);
        if (!(s.state in WANTS) || s.kind === 'shell') { armed.delete(s.id); continue; }
        if (seen.has(s.id)) continue;
        if (armed.get(s.id) === s.state) continue;
        armed.set(s.id, s.state);
        out.push({ sid: s.id, title: s.label, body: s.attention?.message || WANTS[s.state] });
      }
      for (const sid of [...armed.keys()]) if (!live.has(sid)) armed.delete(sid);
      return out;
    },
  };
}

export function titleText(count) {
  return count > 0 ? `(${count}) claudio` : 'claudio';
}
