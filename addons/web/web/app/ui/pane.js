// Panes: 1, 2 or 2x2. Each shows one session: header, xterm, status strip (and the inline composer if the setting is on).
import { html } from 'htm/preact';
import { Component } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { model, conn } from '../store.js';
import { createTerm, refitTerminal } from '../term.js';
import { Glyph } from './glyph.js';
import { Branch, Strip } from './strip.js';
import { Composer } from './composer.js';
import { maximized, toggleMaximize } from './maximize.js';
import { settings } from '../settings.js';
import { ApproveButtons } from './approve.js';
import { SessionActions, dropSession, focusPane, resumeSession, closePane } from './session-actions.js';
import { SID_TYPE } from './rail.js';

export { drawer, openDrawer } from './session-actions.js';

// Never re-rendered: xterm owns everything inside.
class TermHost extends Component {
  shouldComponentUpdate() { return false; }
  render({ hostRef }) {
    return html`<div class="term-host" data-testid="terminal" ref=${hostRef}></div>`;
  }
}

function TermView({ s, focus, onFit, onAttached }) {
  const hostRef = useRef(null);
  const handle = useRef(null);
  const focusRef = useRef(focus);
  focusRef.current = focus;
  useEffect(() => {
    const h = createTerm({ sid: s.id, el: hostRef.current, isFocused: () => focusRef.current, onAttached, onFit });
    handle.current = h;
    return () => { handle.current = null; h.dispose(); };
  }, [s.id]);
  // the socket came back: the server forgot our attachment
  const open = conn.value === 'open';
  const was = useRef(open);
  useEffect(() => {
    if (open && !was.current) handle.current?.attach();
    was.current = open;
  }, [open]);
  // a pane that gains focus takes over the PTY size
  useEffect(() => { if (focus) handle.current?.refit(); }, [focus]);
  return html`<${TermHost} hostRef=${hostRef} />`;
}

function PaneSession({ s, i, focus }) {
  const [fit, setFit] = useState(null);
  const [att, setAtt] = useState(null);
  useEffect(() => { setFit(null); setAtt(null); }, [s.id]);
  // Only a size set by another client is flagged; this tab's own panes letterbox silently.
  const foreign = !!(att && att.foreign);
  const dims = att ? ` · ${att.cols}×${att.rows}` : '';
  return html`
    <header class="pane-head">
      <span class="pane-no">${i + 1}</span><${Glyph} s=${s} />
      <span class="pane-title" title=${`${s.label}${dims}`}>${s.label}</span>
      ${s.host !== 'local' ? html`<span class="chip">${s.host}</span>` : null}
      <${Branch} s=${s} />
      ${foreign ? html`<span class="badge badge-foreign" data-testid="foreign-size" onClick=${() => { focusPane(i); setTimeout(() => refitTerminal(s.id), 0); }} title=${`Another client set this terminal to ${att.cols}x${att.rows}${fit ? `; this pane fits ${fit.cols}x${fit.rows}` : ''}. Click here to take over its size.`}>PTY ${att.cols}×${att.rows}</span>` : null}
      <span class="topbar-spacer"></span>
      ${maximized.value === i ? html`<button type="button" class="btn btn-sm" data-testid="fullscreen-exit" title="Back to the layout (Alt+Shift+F)" onClick=${() => toggleMaximize(i)}>Exit full window</button>` : null}
      <${ApproveButtons} s=${s} />
      <${SessionActions} s=${s} pane=${i} />
    </header>
    ${s.dormant ? html`<div class="pane-dormant">
        <div class="coin">DORMANT</div>
        <p class="muted">${s.label} has no process. Resuming starts claude again and continues its conversation.</p>
        <button class="btn btn-primary" data-testid="resume-btn" onClick=${() => resumeSession(s)}>Resume</button>
      </div>`
      : html`<${TermView} s=${s} focus=${focus} onFit=${setFit} onAttached=${setAtt} />`}
    ${s.kind === 'claude' && settings.value.inlineComposer ? html`<${Composer} key=${s.id} s=${s} />` : null}
    <${Strip} s=${s} />`;
}

export function Pane({ i }) {
  const m = model.value;
  const lay = m.layout;
  const focus = lay.focus === i;
  const sid = lay.panes[i] ?? null;
  const s = sid ? m.sessions[sid] : null;
  const [over, setOver] = useState(false);
  return html`<section class="pane" data-testid="pane" data-pane=${i} data-focus=${focus} data-max=${maximized.value === i} data-state=${s?.state ?? 'empty'} data-over=${over}
    onPointerDownCapture=${() => focusPane(i)}
    onDragOver=${(e) => { if (e.dataTransfer.types.includes(SID_TYPE)) { e.preventDefault(); if (!over) setOver(true); } }}
    onDragLeave=${(e) => { if (!e.currentTarget.contains(e.relatedTarget)) setOver(false); }}
    onDrop=${(e) => { setOver(false); const d = e.dataTransfer.getData(SID_TYPE); if (d) { e.preventDefault(); e.stopPropagation(); dropSession(i, d); } }}>
    ${s ? html`<${PaneSession} key=${s.id} s=${s} i=${i} focus=${focus} />`
      : html`<header class="pane-head"><span class="pane-no">${i + 1}</span><span class="muted">empty</span></header>
        <div class="pane-body"><div class="pane-empty">
          <div class="coin">DROP A SESSION HERE</div>
          <div class="hint"><span>drag a card from the rail</span><span>or click one to open it here</span></div>
        </div></div>`}
  </section>`;
}

export function PaneGrid() {
  const lay = model.value.layout;
  return html`<main class="panes" data-testid="panes" data-grid=${lay.grid}>
    ${Array.from({ length: lay.grid }, (_, i) => html`<${Pane} key=${i} i=${i} />`)}
  </main>`;
}
