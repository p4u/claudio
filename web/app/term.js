// xterm.js owned by a pane through a ref: Preact never re-renders what lives in `el`.
// One handle per mounted pane. OSC 52 clipboard writes are ignored (no clipboard addon) and
// every link asks before it opens (docs/web-protocol.md §7).
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { SearchAddon } from '@xterm/addon-search';
import { conn, req, sendInput, onOutput, onServerEvent } from './store.js';
import { shouldSendResize, resizePolicy } from './panes.js';
import { confirm } from './ui/modal.js';
import { createLatexLayer } from './ui/latex-layer.js';

const handles = new Map(); // sid -> handle

export function focusTerminal(sid) {
  const h = handles.get(sid);
  h?.focus();
  return !!h;
}

export function refitTerminal(sid) {
  handles.get(sid)?.refit();
}

export function termSize(sid) {
  const h = handles.get(sid);
  return h ? { rows: h.term.rows, cols: h.term.cols } : null;
}

const ANSI = {
  dark: {
    black: '#2a3441', red: '#ff5c5f', green: '#3ddc84', yellow: '#f5b32b', blue: '#6ea8ff', magenta: '#c792ea', cyan: '#22d3ee', white: '#d5dde5',
    brightBlack: '#5c6b7a', brightRed: '#ff8082', brightGreen: '#6ff0a6', brightYellow: '#ffd36b', brightBlue: '#9cc2ff', brightMagenta: '#dcb4f5', brightCyan: '#6be6f7', brightWhite: '#ffffff',
  },
  light: {
    black: '#1a2028', red: '#c4222a', green: '#12793b', yellow: '#86560a', blue: '#1f58c8', magenta: '#8b3fb5', cyan: '#0a7388', white: '#6b7280',
    brightBlack: '#566170', brightRed: '#e0393f', brightGreen: '#1a9a4e', brightYellow: '#a06a10', brightBlue: '#3b74e6', brightMagenta: '#a557d1', brightCyan: '#1590a8', brightWhite: '#1a2028',
  },
};

function cssVar(name, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function look() {
  const light = document.documentElement.dataset.theme?.endsWith('light');
  const size = parseInt(cssVar('--term-font-size', '14'), 10) || 14;
  return {
    fontSize: size,
    fontFamily: cssVar('--font-ui', 'monospace'),
    theme: {
      ...ANSI[light ? 'light' : 'dark'],
      background: cssVar('--term-bg', '#0b0f14'),
      foreground: cssVar('--text', '#e6edf3'),
      cursor: cssVar('--accent', '#8b7bff'),
      cursorAccent: cssVar('--term-bg', '#0b0f14'),
      selectionBackground: light ? 'rgba(91,67,224,0.28)' : 'rgba(139,123,255,0.38)',
    },
  };
}

export async function openLink(uri) {
  let url;
  try { url = new URL(uri); } catch { return; }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') return;
  const ok = await confirm({ title: 'Open link?', body: url.href, yes: 'Open', no: 'Cancel' });
  if (ok) window.open(url.href, '_blank', 'noopener');
}

// opts: {sid, el, isFocused(): bool, onAttached({rows, cols})}
export function createTerm({ sid, el, isFocused, onAttached, onFit }) {
  let disposed = false;
  let opened = false;
  let lastSent = null; // the size the PTY is believed to have
  let mine = null; // the size THIS tab last asked for (attach or resize)
  let wantFocus = false; // focus() came before the terminal was opened
  let timer = null;
  let raf = 0;
  const fit = new FitAddon();
  const search = new SearchAddon();
  const term = new Terminal({
    allowProposedApi: true,
    cursorBlink: true,
    scrollback: 5000,
    linkHandler: { activate: (_ev, text) => { openLink(text); }, allowNonHttpProtocols: false },
    ...look(),
  });
  term.loadAddon(fit);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = '11';
  term.loadAddon(new WebLinksAddon((_ev, uri) => { openLink(uri); }));
  term.loadAddon(search);
  const latex = createLatexLayer(term, sid);

  const size = () => ({ rows: term.rows, cols: term.cols });
  const hasBox = () => el.clientWidth > 8 && el.clientHeight > 8;

  // The xterm grid always equals the PTY size this pane last learned; only the focused pane
  // changes the PTY (and then the grid follows what was sent).
  function proposed() {
    if (!hasBox()) return null;
    try {
      const d = fit.proposeDimensions();
      return d && d.rows > 0 && d.cols > 0 ? { rows: d.rows, cols: d.cols } : null;
    } catch { return null; }
  }

  function sendResize() {
    timer = null;
    if (disposed || conn.value !== 'open') return;
    const s = proposed();
    if (!s || !shouldSendResize({ focused: isFocused(), cause: 'fit', size: s, lastSent })) return;
    lastSent = s;
    mine = s;
    term.resize(s.cols, s.rows);
    onAttached?.({ ...s, foreign: false });
    req('session.resize', { sid, ...s }).catch(() => {});
  }

  function refit() {
    if (disposed || !opened) return;
    const s = proposed();
    if (!s) return;
    onFit?.(s);
    if (!shouldSendResize({ focused: isFocused(), cause: 'fit', size: s, lastSent })) return;
    clearTimeout(timer);
    timer = setTimeout(sendResize, resizePolicy().debounceMs);
  }

  function attach() {
    if (disposed || !opened || conn.value !== 'open') return;
    const s = proposed() ?? size();
    term.resize(s.cols, s.rows);
    lastSent = s;
    mine = s;
    onAttached?.({ ...s, foreign: false });
    onFit?.(s);
    req('session.attach', { sid, ...s }).catch(() => {});
  }

  const offOut = onOutput(sid, (bytes) => { if (!disposed) term.write(bytes); });
  const offEv = onServerEvent((ev) => {
    if (ev.ev !== 'attached' || ev.sid !== sid || disposed) return;
    term.resize(ev.cols, ev.rows);
    term.reset(); // the next output frame is the snapshot
    lastSent = { rows: ev.rows, cols: ev.cols };
    // Foreign only when the size is not the one this tab asked for: another client sized the PTY.
    // A pane here that merely differs from the focused pane's fit just letterboxes.
    const foreign = !!mine && (mine.rows !== ev.rows || mine.cols !== ev.cols);
    onAttached?.({ rows: ev.rows, cols: ev.cols, foreign });
  });

  const d1 = term.onData((d) => sendInput(sid, d));
  const d2 = term.onBinary((d) => sendInput(sid, Uint8Array.from(d, (c) => c.charCodeAt(0) & 0xff)));

  // copy on select: when the pointer is released over a selection
  const copy = () => {
    const t = term.getSelection();
    if (t) navigator.clipboard?.writeText(t).catch(() => {});
  };
  el.addEventListener('pointerup', copy);

  const ro = new ResizeObserver(() => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; refit(); });
  });

  const mo = new MutationObserver(() => {
    const l = look();
    term.options.theme = l.theme;
    if (term.options.fontSize !== l.fontSize) term.options.fontSize = l.fontSize;
    refit();
    latex.schedule();
  });

  const fontReady = document.fonts?.load ? document.fonts.load(`${look().fontSize}px "JetBrains Mono"`).catch(() => {}) : Promise.resolve();
  fontReady.then(() => {
    if (disposed) return;
    term.open(el);
    opened = true;
    latex.attach();
    if (wantFocus) { wantFocus = false; term.focus(); }
    ro.observe(el);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'style'] });
    attach();
  });

  const handle = {
    sid, term, attach, refit,
    focus() { if (opened) term.focus(); else wantFocus = true; },
    findNext(q) { return search.findNext(q); },
    findPrevious(q) { return search.findPrevious(q); },
    dispose() {
      if (disposed) return;
      disposed = true;
      clearTimeout(timer);
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect(); mo.disconnect(); latex.dispose();
      el.removeEventListener('pointerup', copy);
      offOut(); offEv(); d1.dispose(); d2.dispose();
      if (handles.get(sid) === handle) handles.delete(sid);
      if (conn.value === 'open') req('session.detach', { sid }).catch(() => {});
      term.dispose();
    },
  };
  handles.set(sid, handle);
  return handle;
}
