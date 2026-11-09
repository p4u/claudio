// Key bindings and the focus model's pure part. Imports nothing.
// Events are KeyboardEvent-like: {key, code, ctrlKey, shiftKey, altKey, metaKey}.
const E = (key, o = {}) => ({ key, code: o.code ?? '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...o });

export const BINDINGS = [
  { keys: 'k', action: 'palette', help: 'command palette', ev: E('k') },
  { keys: 'Ctrl+K', action: 'palette-ctrl', help: 'command palette (also in text fields; not in the terminal)', ev: E('k', { ctrlKey: true }), hide: true },
  { keys: 'a', action: 'next-attention', help: 'jump to the next session that needs you', ev: E('a') },
  { keys: 'n', action: 'new-session', help: 'new claude session', ev: E('n') },
  { keys: 't', action: 'new-terminal', help: 'new terminal tab', ev: E('t') },
  { keys: '/', action: 'search', help: 'search conversations', ev: E('/') },
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
  '/': 'search', k: 'palette', a: 'next-attention', n: 'new-session', t: 'new-terminal',
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
// Alt+letter is not here: the TUI's own bindings use exactly these (TUI_BINDINGS).
export function forbidden(ev) {
  const k = String(ev.key).toLowerCase();
  if (ev.ctrlKey && !ev.altKey && (k === 'w' || k === 't' || k === 'n')) return true;
  if (ev.ctrlKey && k === 'tab') return true;
  return false;
}

// ---- TUI parity: the manager's Alt keys (src/tui/keymap.rs), active everywhere ----
// Matched on `code` (layout independent). `action` names are run through palette.js's runAction.
const A = (keys, code, action, help, o = {}) => ({ keys, code, action, help, ev: { key: o.key ?? code, code, ctrlKey: false, shiftKey: !!o.shift, altKey: true, metaKey: false } });
const DIGIT_HELP = 'go to session 1…9, 0 for 10 (rail order)';
export const TUI_BINDINGS = [
  A('Alt+←', 'ArrowLeft', 'session-prev', 'previous session in the focused pane'),
  A('Alt+→', 'ArrowRight', 'session-next', 'next session in the focused pane'),
  ...[1, 2, 3, 4, 5, 6, 7, 8, 9, 0].map((d) => A(`Alt+Shift+${d}`, `Digit${d}`, `goto-session-${d === 0 ? 10 : d}`, DIGIT_HELP, { shift: true, key: ')!@#$%^&*('[d] })),
  A('Alt+n', 'KeyN', 'new-session', 'new claude session'),
  A('Alt+c', 'KeyC', 'terminal-beside', 'terminal beside, in the session\'s folder'),
  A('Alt+r', 'KeyR', 'rename-focused', 'rename the focused session'),
  A('Alt+x', 'KeyX', 'kill-focused', 'kill the focused session (asks)'),
  A('Alt+e', 'KeyE', 'restart-focused', 'restart the focused session'),
  A('Alt+a', 'KeyA', 'next-attention', 'next session that needs you'),
  A('Alt+g', 'KeyG', 'board', 'the board (overview)'),
  A('Alt+s', 'KeyS', 'stats', 'addon panel (proxy stats)'),
  A('Alt+l', 'KeyL', 'history', 'history drawer'),
  A('Alt+u', 'KeyU', 'reader', 'reader drawer'),
  A('Alt+i', 'KeyI', 'extensions', 'extensions screen'),
  A('Alt+h', 'KeyH', 'help', 'this help'),
  A('Alt+z', 'KeyZ', 'palette', 'command palette (the extension actions)'),
  // Web-only. Claude Code uses Alt+P, Alt+T, Alt+M and readline's Alt+B/F/D/Enter, so Alt+O and
  // Alt+Shift+F (not Alt+F) are free; the TUI has no key for either.
  A('Alt+o', 'KeyO', 'compose', 'compose a prompt in an editor (Ctrl+Enter sends)'),
  A('Alt+Shift+f', 'KeyF', 'fullscreen-pane', 'fill the window with the focused pane', { shift: true, key: 'F' }),
];

const BY_CODE = new Map(TUI_BINDINGS.filter((b) => !b.ev.shiftKey).map((b) => [b.code, b.action]));
const BY_SHIFT = new Map(TUI_BINDINGS.filter((b) => b.ev.shiftKey).map((b) => [b.code, b.action]));

// The action a TUI parity key runs, or null. AltGr (Ctrl+Alt) and Meta never match.
export function tuiBinding(ev) {
  if (!ev.altKey || ev.ctrlKey || ev.metaKey) return null;
  const table = ev.shiftKey ? BY_SHIFT : BY_CODE;
  return table.get(ev.code) ?? null;
}

// ---- what a terminal gets for a key ----
// Shift+Enter is claude's newline: ESC CR, the same as Alt+Enter (xterm sends a bare CR).
export function shiftEnterBytes(ev) {
  return ev.key === 'Enter' && ev.shiftKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey ? '\x1b\r' : null;
}

const NAMED = { Enter: '\r', Escape: '\x1b', Tab: '\t', Backspace: '\x7f', ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D' };

// An approximation of what xterm.js sends, for the key check; null when the key sends nothing.
export function terminalBytes(ev) {
  if (ev.metaKey) return null;
  const se = shiftEnterBytes(ev);
  if (se) return se;
  if (ev.key === 'Tab' && ev.shiftKey) return '\x1b[Z';
  let base = Object.hasOwn(NAMED, ev.key) ? NAMED[ev.key] : null;
  if (base === null && ev.key.length === 1) {
    base = ev.key;
    if (ev.ctrlKey && /^[a-z]$/i.test(ev.key)) base = String.fromCharCode(ev.key.toUpperCase().charCodeAt(0) - 64);
    else if (ev.ctrlKey) return null;
  }
  if (base === null) return null;
  return ev.altKey ? '\x1b' + base : base;
}

export function escapeBytes(b) {
  return [...b].map((c) => {
    const n = c.codePointAt(0);
    if (n === 0x5c) return '\\\\';
    return n < 0x20 || n === 0x7f ? `\\x${n.toString(16).padStart(2, '0')}` : c;
  }).join('');
}

// One line for the help's key check: the key, its code, modifiers, and what claudio does with it.
export function describeKey(ev) {
  const mods = ['ctrl', 'alt', 'shift', 'meta'].filter((m) => ev[m + 'Key']).join('+') || 'none';
  const head = `key=${ev.key} code=${ev.code} mods=${mods}`;
  const act = tuiBinding(ev);
  if (act) return `${head} → claudio: ${act}`;
  if (isLeaveTerminal(ev)) return `${head} → claudio: leave-terminal`;
  const b = terminalBytes(ev);
  return b === null ? `${head} → goes to the page` : `${head} → sent to the session: ${escapeBytes(b)}`;
}
