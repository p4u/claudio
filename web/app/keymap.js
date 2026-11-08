// Key bindings and the focus model's pure part. Imports nothing.
// Events are KeyboardEvent-like: {key, code, ctrlKey, shiftKey, altKey, metaKey}.
const E = (key, o = {}) => ({ key, code: o.code ?? '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...o });

export const BINDINGS = [
  { keys: 'k', action: 'palette', help: 'command palette', ev: E('k') },
  { keys: 'Ctrl+K', action: 'palette-ctrl', help: 'command palette (also while typing)', ev: E('k', { ctrlKey: true }), hide: true },
  { keys: 'a', action: 'next-attention', help: 'jump to the next session that needs you', ev: E('a') },
  { keys: 'n', action: 'new-session', help: 'new claude session', ev: E('n') },
  { keys: 't', action: 'new-terminal', help: 'new terminal tab', ev: E('t') },
  { keys: '1', action: 'focus-pane-1', help: 'focus pane 1', ev: E('1') },
  { keys: '2', action: 'focus-pane-2', help: 'focus pane 2', ev: E('2') },
  { keys: '3', action: 'focus-pane-3', help: 'focus pane 3', ev: E('3') },
  { keys: '4', action: 'focus-pane-4', help: 'focus pane 4', ev: E('4') },
  { keys: '↓', action: 'rail-next', help: 'next session in the rail', ev: E('ArrowDown') },
  { keys: '↑', action: 'rail-prev', help: 'previous session in the rail', ev: E('ArrowUp') },
  { keys: 'r', action: 'reader', help: 'reader: last answers', ev: E('r') },
  { keys: 'g', action: 'changes', help: 'git changes', ev: E('g') },
  { keys: '?', action: 'help', help: 'this help', ev: E('?', { shiftKey: true }) },
  { keys: 'Enter', action: 'enter-terminal', help: 'type into the focused terminal', ev: E('Enter') },
  { keys: 'Esc', action: 'close-overlay', help: 'close the open panel', ev: E('Escape') },
  { keys: 'Ctrl+Shift+Space', action: 'leave-terminal', help: 'leave the terminal (the only key the app takes there)', ev: E(' ', { code: 'Space', ctrlKey: true, shiftKey: true }) },
];

const SINGLE = {
  k: 'palette', a: 'next-attention', n: 'new-session', t: 'new-terminal',
  1: 'focus-pane-1', 2: 'focus-pane-2', 3: 'focus-pane-3', 4: 'focus-pane-4',
  ArrowDown: 'rail-next', ArrowUp: 'rail-prev', r: 'reader', g: 'changes',
  '?': 'help', Enter: 'enter-terminal', Escape: 'close-overlay',
};

export function isLeaveTerminal(ev) {
  return !!ev.ctrlKey && !!ev.shiftKey && !ev.altKey && !ev.metaKey && (ev.code === 'Space' || ev.key === ' ');
}

export function appAction(ev, ctx = {}) {
  if (ev.altKey || ev.metaKey) return null;
  if (ev.ctrlKey) return !ev.shiftKey && ev.key.toLowerCase() === 'k' ? 'palette' : null;
  if (ctx.typing) return null;
  if (ctx.overlay) return ev.key === 'Escape' ? 'close-overlay' : null;
  return Object.hasOwn(SINGLE, ev.key) ? SINGLE[ev.key] : null;
}

// Keys the app must never bind: they collide with the browser or with a terminal's own use.
export function forbidden(ev) {
  const k = String(ev.key).toLowerCase();
  if (ev.altKey && /^[a-z]$/.test(k)) return true;
  if (ev.ctrlKey && !ev.altKey && (k === 'w' || k === 't' || k === 'n')) return true;
  if (ev.ctrlKey && k === 'tab') return true;
  return false;
}
