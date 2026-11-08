// The hover info: pointing at (or focusing) a rail item or a board row for 800 ms shows its facts in
// a popover beside it. fixed position + pointer-events none: it never moves or covers anything.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { model } from '../store.js';
import { stateClass } from '../queue.js';
import { infoRows, placeTip, INFO_DELAY_MS } from '../info.js';
import { now } from './glyph.js';
import { contextWindow } from '../gauge.js';

const tip = signal(null); // {sid, rect, vertical}
let timer = null;

export function hideInfo() {
  clearTimeout(timer);
  timer = null;
  if (tip.value) tip.value = null;
}

function arm(sid, el, vertical) {
  hideInfo();
  timer = setTimeout(() => {
    timer = null;
    if (!el.isConnected) return;
    const r = el.getBoundingClientRect();
    tip.value = { sid, rect: { left: r.left, top: r.top, right: r.right, bottom: r.bottom }, vertical };
  }, INFO_DELAY_MS);
}

// Props to spread on an item. `vertical`: a wide row, the popover goes below or above it.
export function infoProps(s, vertical = false) {
  return {
    onPointerEnter: (e) => { if (e.pointerType !== 'touch') arm(s.id, e.currentTarget, vertical); },
    onPointerLeave: hideInfo,
    onPointerDown: hideInfo,
    onDragStart: hideInfo,
    onFocus: (e) => { if (e.target === e.currentTarget && e.currentTarget.matches(':focus-visible')) arm(s.id, e.currentTarget, vertical); },
    onBlur: hideInfo,
  };
}

if (typeof document !== 'undefined') {
  document.addEventListener('keydown', hideInfo, true);
  window.addEventListener('blur', hideInfo);
}

export function InfoPopover() {
  const t = tip.value;
  const ref = useRef(null);
  const [pos, setPos] = useState(null);
  const s = t ? model.value.sessions[t.sid] : null;
  useLayoutEffect(() => {
    if (!t || !ref.current) { setPos(null); return; }
    const b = ref.current.getBoundingClientRect();
    setPos(placeTip(t.rect, { w: b.width, h: b.height }, { w: window.innerWidth, h: window.innerHeight }, 8, t.vertical));
  }, [t]);
  if (!t || !s) return null;
  const win = contextWindow(s.model, s.context_tokens);
  const rows = infoRows(s, stateClass(s), now.value / 1000, win);
  return html`<div class="info-pop popover" data-testid="session-info" data-sid=${s.id} role="tooltip" ref=${ref}
    style=${pos ? `left:${pos.left}px;top:${pos.top}px` : 'left:0;top:0;visibility:hidden'}>
    <dl class="info-list">
      ${rows.map((r) => html`<div class="info-row"><dt>${r.k}</dt><dd>${r.v}</dd></div>`)}
    </dl>
  </div>`;
}
