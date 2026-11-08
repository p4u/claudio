// <latex> formulas over a terminal pane. The terminal stays the source of truth: this only lays
// absolutely positioned boxes (pointer-events: none) over xterm's screen, never touching its
// buffer or the PTY. Matching is app/latex-overlay.js; the TUI does the same with its one-line form.
import { effect } from '@preact/signals';
import DOMPurify from 'dompurify';
import { model } from '../store.js';
import { settings } from '../settings.js';
import { getKatex } from '../render/katex.js';
import { KATEX_OPTIONS } from '../render/markdown.js';
import { findSpans, formulaKeys, formulasOf, matchFormula } from '../latex-overlay.js';

const MIN_SCALE = 0.5;
const MAX_HEIGHT = 1.6; // of a cell
const CACHE_MAX = 256;

// source -> sanitized KaTeX html, or null when KaTeX refused it
const typeset = new Map();
async function rendered(src) {
  if (typeset.has(src)) return typeset.get(src);
  let out = null;
  try {
    const katex = await getKatex();
    out = DOMPurify.sanitize(katex.renderToString(src, { ...KATEX_OPTIONS, displayMode: false }));
  } catch { out = null; }
  if (typeset.size >= CACHE_MAX) typeset.delete(typeset.keys().next().value);
  typeset.set(src, out);
  return out;
}

const div = (cls) => { const e = document.createElement('div'); e.className = cls; return e; };

// Read the visible rows into the shape findSpans wants.
function visibleRows(term) {
  const buf = term.buffer.active;
  const out = [];
  const cell = buf.getNullCell();
  for (let y = 0; y < term.rows; y++) {
    const line = buf.getLine(buf.viewportY + y);
    const glyphs = [];
    if (line) {
      for (let x = 0; x < term.cols; x++) {
        const c = line.getCell(x, cell);
        if (!c || c.getWidth() === 0) continue;
        glyphs.push({ col: x, ch: c.getChars() || ' ' });
      }
    }
    out.push({ glyphs, width: term.cols });
  }
  return out;
}

function screenText(term) {
  const buf = term.buffer.active;
  const lines = [];
  for (let y = 0; y < term.rows; y++) lines.push(buf.getLine(buf.viewportY + y)?.translateToString(true) ?? '');
  return lines;
}

export function createLatexLayer(term, sid) {
  let layer = null;
  let screenEl = null;
  let disposed = false;
  let raf = 0;
  let lastSig = null;
  let keys = null;
  let keysFor = null;
  const boxes = new Map(); // key -> {els: [], dead: bool}

  function clear() {
    for (const b of boxes.values()) { b.dead = true; for (const e of b.els) e.remove(); }
    boxes.clear();
    lastSig = null;
  }

  function schedule() {
    if (disposed || raf) return;
    raf = requestAnimationFrame(() => { raf = 0; update(); });
  }

  function place(el, row, col, endCol, cellW, cellH) {
    el.style.left = `${col * cellW}px`;
    el.style.top = `${row * cellH}px`;
    el.style.width = `${(endCol - col) * cellW}px`;
    el.style.height = `${cellH}px`;
  }

  async function build(entry, key, span, src, cellW, cellH) {
    const first = div('latex-box');
    first.dataset.testid = 'latex-box';
    first.style.visibility = 'hidden';
    const lastCol = span.cont.length ? term.cols : span.endCol;
    place(first, span.row, span.col, lastCol, cellW, cellH);
    const blanks = span.cont.map((c) => {
      const b = div('latex-blank');
      place(b, c.row, c.col, c.endCol, cellW, cellH);
      return b;
    });
    const html = await rendered(src);
    if (entry.dead || disposed || !html) { if (!entry.dead) entry.els = []; return; }
    const inner = document.createElement('span');
    inner.className = 'latex-inner';
    inner.style.fontSize = `${term.options.fontSize}px`;
    inner.innerHTML = html; // DOMPurify'd above
    first.append(inner);
    layer.append(...blanks, first);
    entry.els = [first, ...blanks];
    try { await document.fonts?.ready; } catch { /* ignore */ }
    if (entry.dead || disposed) return;
    const w = inner.offsetWidth;
    const h = inner.offsetHeight;
    const boxW = first.clientWidth;
    const scale = Math.min(1, boxW / (w || 1), (MAX_HEIGHT * cellH) / (h || 1));
    if (!w || !h || scale < MIN_SCALE) { // does not fit: the raw tag stays
      for (const e of entry.els) e.remove();
      entry.els = [];
      return;
    }
    if (scale < 1) inner.style.transform = `scale(${scale})`;
    first.style.visibility = '';
  }

  function update() {
    if (disposed || !screenEl || !term.element) return;
    const on = settings.value.renderFormulas;
    const formulas = on ? formulasOf(model.value.lens[sid]) : [];
    const lines = on && formulas.length ? screenText(term) : [];
    const cellW = screenEl.clientWidth / term.cols;
    const cellH = screenEl.clientHeight / term.rows;
    const hasTag = lines.some((l) => l.includes('<latex>'));
    const sig = hasTag ? `${cellW}|${cellH}|${term.options.fontSize}|${formulas.join('\u0001')}|${lines.join('\n')}` : '';
    if (sig === lastSig) return;
    lastSig = sig;
    if (!hasTag || !(cellW > 0) || !(cellH > 0)) { clear(); lastSig = sig; return; }
    if (keysFor !== formulas.join('\u0001')) { keys = formulas.map(formulaKeys); keysFor = formulas.join('\u0001'); }
    const want = new Map();
    for (const span of findSpans(visibleRows(term))) {
      const src = matchFormula(formulas, span.text, keys);
      if (src === null) continue;
      const key = `${span.row},${span.col},${span.endRow},${span.endCol},${cellW},${cellH}|${src}`;
      want.set(key, { span, src });
    }
    for (const [key, b] of boxes) {
      if (want.has(key)) continue;
      b.dead = true;
      for (const e of b.els) e.remove();
      boxes.delete(key);
    }
    for (const [key, { span, src }] of want) {
      if (boxes.has(key)) continue;
      const entry = { els: [], dead: false };
      boxes.set(key, entry);
      build(entry, key, span, src, cellW, cellH);
    }
  }

  const subs = [];
  const stop = effect(() => { settings.value; model.value.lens[sid]; schedule(); });

  return {
    // after term.open(): the terminal's screen element exists
    attach() {
      if (disposed || layer) return;
      screenEl = term.element?.querySelector('.xterm-screen');
      if (!screenEl) return;
      layer = div('latex-layer');
      layer.dataset.testid = 'latex-overlay';
      screenEl.append(layer);
      subs.push(term.onRender(schedule), term.onScroll(schedule));
      schedule();
    },
    schedule,
    dispose() {
      if (disposed) return;
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      stop();
      for (const s of subs) s.dispose();
      clear();
      layer?.remove();
    },
  };
}
