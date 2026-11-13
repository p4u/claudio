// xterm.js owned by a pane through a ref: Preact never re-renders what lives in `el`.
// One handle per mounted pane. OSC 52 clipboard writes are ignored (no clipboard addon) and
// every link asks before it opens (docs/web-protocol.md §7).
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { SearchAddon } from '@xterm/addon-search';
import { tuiBinding, shiftEnterBytes } from './keymap.js';
import { conn, req, sendInput, onOutput, onServerEvent } from './store.js';
import { shouldSendResize, resizePolicy } from './panes.js';
import { confirm } from './ui/modal.js';
import { createLatexLayer } from './ui/latex-layer.js';

const handles = new Map(); // sid -> handle
const MODIFIERS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'AltGraph', 'CapsLock']);

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

// The ANSI set and the terminal colours live in tokens.css (--ansi-*, --term-*): plain colours,
// derived from DESIGN.md's palette.
const ANSI = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'];

function cssVar(name, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function look() {
  const size = parseInt(cssVar('--term-font-size', '14'), 10) || 14;
  const theme = {};
  for (const c of ANSI) {
    theme[c] = cssVar(`--ansi-${c}`);
    theme[`bright${c[0].toUpperCase()}${c.slice(1)}`] = cssVar(`--ansi-bright-${c}`);
  }
  return {
    fontSize: size,
    fontFamily: cssVar('--font-mono', 'monospace'),
    fontWeight: 400,
    fontWeightBold: 700, // claude's bold output: JetBrains Mono Bold, the only bold in the app
    theme: {
      ...theme,
      background: cssVar('--term-bg'),
      foreground: cssVar('--term-fg'),
      cursor: cssVar('--term-cursor'),
      cursorAccent: cssVar('--term-bg'),
      selectionBackground: cssVar('--term-selection'),
      // The scrollbar is always shown (layout.css): a thumb in the hairline tokens.
      scrollbarSliderBackground: cssVar('--border-strong'),
      scrollbarSliderHoverBackground: cssVar('--text-muted'),
      scrollbarSliderActiveBackground: cssVar('--text-muted'),
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
    measure();
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

  // The pane in use takes the PTY's size back from another client (tmux's "latest"): a key or a
  // paste in it, a click in it, its window coming back. Never on onData, which also carries the
  // terminal's own replies (focus reports, device attributes): two panes would trade the size.
  // Nothing when the PTY already has this pane's fit.
  function claim() {
    if (disposed || !opened || !isFocused()) return;
    const s = proposed();
    if (!s || (lastSent && lastSent.rows === s.rows && lastSent.cols === s.cols)) return;
    clearTimeout(timer);
    sendResize();
  }

  // A grid taller than its box (another client's size) is shifted up just enough to show the
  // cursor row, where claude's prompt is; never past the grid's top, so a prompt near the top of
  // a fresh terminal stays in view too. Measured on every resize, applied on every cursor move.
  let over = 0; // rows of the grid below the box
  let cellH = 0;
  let shift = 0;
  function measure() {
    if (disposed || !opened) return;
    const screen = term.element?.querySelector('.xterm-screen');
    const cs = getComputedStyle(el);
    const room = el.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom);
    cellH = screen && term.rows ? screen.offsetHeight / term.rows : 0;
    over = cellH > 0 ? Math.max(0, term.rows - Math.floor(room / cellH)) : 0;
    follow();
  }
  function follow() {
    const rows = over > 0 ? Math.min(over, Math.max(0, term.buffer.active.cursorY + 1 - (term.rows - over))) : 0;
    if (rows === shift) return;
    shift = rows;
    if (term.element) term.element.style.marginTop = rows ? `${-rows * cellH}px` : '';
  }

  // A move to a screen of another density keeps the box's CSS size (no ResizeObserver call) but
  // can change the cell size: refit, and watch the new density.
  let dpr = null;
  const onDpr = () => { watchDpr(); refit(); };
  function watchDpr() {
    dpr?.removeEventListener('change', onDpr);
    dpr = globalThis.matchMedia?.(`(resolution: ${globalThis.devicePixelRatio || 1}dppx)`) ?? null;
    dpr?.addEventListener?.('change', onDpr);
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
    measure();
    lastSent = { rows: ev.rows, cols: ev.cols };
    // Foreign only when the size is not the one this tab asked for: another client sized the PTY.
    // A pane here that merely differs from the focused pane's fit just letterboxes.
    const foreign = !!mine && (mine.rows !== ev.rows || mine.cols !== ev.cols);
    onAttached?.({ rows: ev.rows, cols: ev.cols, foreign });
  });

  // Keys the app takes first (document capture already did, this is the belt) and Shift+Enter:
  // claude's newline is ESC CR, which xterm does not send for Shift.
  term.attachCustomKeyEventHandler((ev) => {
    if (tuiBinding(ev)) return false;
    if (ev.type === 'keydown' && !MODIFIERS.has(ev.key)) claim();
    const nl = shiftEnterBytes(ev);
    if (!nl) return true;
    ev.preventDefault();
    if (ev.type === 'keydown') sendInput(sid, nl);
    return false;
  });

  const d3 = term.onCursorMove(follow);
  const d1 = term.onData((d) => sendInput(sid, d));
  const d2 = term.onBinary((d) => sendInput(sid, Uint8Array.from(d, (c) => c.charCodeAt(0) & 0xff)));

  // copy on select: when the pointer is released over a selection
  const copy = () => {
    const t = term.getSelection();
    if (t) navigator.clipboard?.writeText(t).catch(() => {});
  };
  el.addEventListener('pointerup', copy);
  el.addEventListener('pointerdown', claim);
  el.addEventListener('paste', claim, true);
  globalThis.addEventListener?.('focus', claim);

  const ro = new ResizeObserver(() => {
    if (raf) return;
    raf = requestAnimationFrame(() => { raf = 0; measure(); refit(); });
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
    watchDpr();
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
      el.removeEventListener('pointerdown', claim);
      el.removeEventListener('paste', claim, true);
      globalThis.removeEventListener?.('focus', claim);
      dpr?.removeEventListener('change', onDpr);
      offOut(); offEv(); d1.dispose(); d2.dispose(); d3.dispose();
      if (handles.get(sid) === handle) handles.delete(sid);
      if (conn.value === 'open') req('session.detach', { sid }).catch(() => {});
      term.dispose();
    },
  };
  handles.set(sid, handle);
  return handle;
}
