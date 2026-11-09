// Pane layout rules (docs/web-protocol.md §4 Layout). Pure, immutable. Imports nothing.
export function emptyLayout() {
  return { grid: 1, panes: [null], focus: 0 };
}

export function setGrid(layout, grid) {
  if (grid !== 1 && grid !== 2 && grid !== 4) return layout;
  const panes = Array.from({ length: grid }, (_, i) => layout.panes[i] ?? null);
  return { grid, panes, focus: Math.min(Math.max(layout.focus, 0), grid - 1) };
}

export function dropOnPane(layout, pane, sid) {
  if (!Number.isInteger(pane) || pane < 0 || pane >= layout.grid) return layout;
  const panes = layout.panes.map((p, i) => (i === pane ? sid : p === sid ? null : p));
  return { ...layout, panes, focus: pane };
}

export function setFocus(layout, pane) {
  if (!Number.isInteger(pane) || pane < 0 || pane >= layout.grid || pane === layout.focus) return layout;
  return { ...layout, focus: pane };
}

export function resizePolicy() {
  return { debounceMs: 250, sendOn: 'own-fit-of-focused-pane', never: 'attached' };
}

// Only the focused pane's own fit resizes the PTY; an `attached` echo never does; and
// the same size is never sent twice.
export function shouldSendResize({ focused, cause, size, lastSent }) {
  if (!focused || cause !== 'fit') return false;
  if (!size || !(size.rows > 0) || !(size.cols > 0)) return false;
  return !lastSent || lastSent.rows !== size.rows || lastSent.cols !== size.cols;
}

// ids with `sid` moved before `before` (null: to the end).
export function moveBefore(ids, sid, before) {
  if (!ids.includes(sid) || sid === before) return [...ids];
  const rest = ids.filter((x) => x !== sid);
  const at = before === null || before === undefined ? rest.length : rest.indexOf(before);
  if (at < 0) return [...ids];
  rest.splice(at, 0, sid);
  return rest;
}

// A new terminal tab goes into the next empty pane after `sid`'s, else a new pane (grid 1 -> 2),
// else it replaces the pane after `sid`'s.
export function placeBeside(layout, sid, newSid) {
  let l = layout;
  let at = l.panes.indexOf(sid);
  if (at < 0) at = l.focus;
  if (l.grid === 1) l = setGrid(l, 2);
  for (let k = 1; k < l.grid; k++) {
    const j = (at + k) % l.grid;
    if (l.panes[j] === null) return { ...l, panes: l.panes.map((p, i) => (i === j ? newSid : p)) };
  }
  const j = (at + 1) % l.grid;
  return { ...l, panes: l.panes.map((p, i) => (i === j ? newSid : p)) };
}
