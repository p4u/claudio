import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, mode, setMode, banner, selected, fresh, conn, serverInfo } from '../store.js';
import { orderedSessions, groupSessions } from '../model.js';
import { appAction, isLeaveTerminal } from '../keymap.js';
import { overlay, defineAction, runAction } from '../palette.js';
import { Topbar, HostChip } from './topbar.js';
import { Toasts } from './toasts.js';
import { Modal, modal, closeModal } from './modal.js';
import { Palette } from './palette.js';
import { Help } from './help.js';
import { Settings } from './settings.js';

const GLYPH = { working: '⠸', needs_approval: '◆', needs_input: '?', idle: '✓', error: '✗', exited: '○', starting: '·', unknown: '·' };
const SPIN = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const STATE_NAME = { working: 'working', needs_approval: 'needs approval', needs_input: 'needs input', idle: 'idle', error: 'error', exited: 'exited', starting: 'starting', unknown: 'reconnecting' };

const spin = signal(0);
const now = signal(Date.now());
const reduced = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
if (!reduced) setInterval(() => { spin.value = (spin.value + 1) % SPIN.length; }, 100);
setInterval(() => { now.value = Date.now(); }, 20000);

export function glyphFor(s) {
  if (s.kind === 'shell' && s.state !== 'exited') return '$';
  if (s.state === 'unknown') return '⇄';
  if (s.state === 'working' && !reduced) return SPIN[spin.value];
  return GLYPH[s.state] ?? '·';
}

export function ago(sec, nowMs = Date.now()) {
  if (!sec) return '';
  const d = Math.max(0, Math.floor(nowMs / 1000 - sec));
  if (d < 60) return `${d}s`;
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h`;
  return `${Math.floor(d / 86400)}d`;
}

export function Glyph({ s }) {
  return html`<span class="glyph" role="img" aria-label=${s.kind === 'shell' ? 'terminal' : STATE_NAME[s.state]}>${glyphFor(s)}</span>`;
}

const asking = (s) => s.state === 'needs_approval' || s.state === 'needs_input';

function SessionCard({ s }) {
  const g = s.git;
  const dirty = g ? g.staged + g.unstaged + g.untracked : 0;
  return html`<button class="card" data-testid="session-card" data-sid=${s.id} data-state=${s.state} data-kind=${s.kind}
    data-fresh=${!!fresh.value[s.id]} aria-current=${selected.value === s.id} onClick=${() => { selected.value = s.id; }}>
    <span class="card-row"><${Glyph} s=${s} /><span class="card-label">${s.label}</span><span class="card-age">${ago(s.state_since, now.value)}</span></span>
    <span class="card-meta">
      ${s.kind === 'shell' ? html`<span class="chip">terminal</span>` : null}
      ${s.branch ? html`<span class="chip">⎇ ${s.branch}${dirty ? ` ±${dirty}` : ''}</span>` : null}
      ${s.model ? html`<span class="chip">${s.model}</span>` : null}
      ${s.context_tokens ? html`<span class="chip">ctx ${Math.round(s.context_tokens / 1000)}k</span>` : null}
    </span>
  </button>`;
}

function QueueCard({ s }) {
  const a = s.attention;
  return html`<div class="qcard" data-testid="queue-card" data-sid=${s.id} data-state=${s.state} data-kind=${a?.kind ?? s.state} data-fresh=${!!fresh.value[s.id]}>
    <div class="card-row"><${Glyph} s=${s} /><span class="card-label">${s.label}</span>
      ${s.host !== 'local' ? html`<span class="chip">${s.host}</span>` : null}<span class="card-age">${ago(s.state_since, now.value)}</span></div>
    <div class="qcard-msg">${a?.detail || a?.message || STATE_NAME[s.state]}</div>
    <div class="qcard-foot"><span class="qcard-kind">${a?.kind ? a.kind.replace(/_/g, ' ') : STATE_NAME[s.state]}${a?.tool ? ` · ${a.tool}` : ''}</span>
      <button class="btn btn-sm" data-testid="open-btn" onClick=${() => { selected.value = s.id; }}>Open</button></div>
  </div>`;
}

export function attentionSessions(m) {
  const list = orderedSessions(m).filter(asking);
  return list.sort((a, b) => (a.state === b.state ? 0 : a.state === 'needs_approval' ? -1 : 1));
}

function Rail() {
  const m = model.value;
  const all = orderedSessions(m);
  const queue = attentionSessions(m);
  const groups = groupSessions(all);
  return html`<nav class="rail" data-testid="rail" aria-label="Sessions">
    <section class="rail-section">
      <div class="section-head"><span>Needs you</span><span>${queue.length}</span></div>
      <div class="queue" data-testid="queue">
        ${queue.length === 0 ? html`<p class="rail-empty">Nothing waiting. All quiet.</p>` : queue.map((s) => html`<${QueueCard} key=${s.id} s=${s} />`)}
      </div>
    </section>
    <section class="rail-section" style="padding-bottom:0"><div class="section-head"><span>Sessions</span><span>${all.length}</span></div></section>
    <div class="rail-groups">
      ${all.length === 0 ? html`<p class="rail-empty">No sessions yet.</p>` : null}
      ${groups.map((h) => html`<details class="host-group" open key=${h.host}>
        <summary><div class="host-title">${m.hosts[h.host] ? html`<${HostChip} host=${m.hosts[h.host]} />` : h.host}</div></summary>
        ${h.groups.map((g) => html`<details class="cwd-group" open key=${g.cwd}>
          <summary><div class="cwd-title" title=${g.cwd}><span></span><span>${g.label}</span></div></summary>
          <div class="cards">${g.sessions.map((s) => html`<${SessionCard} key=${s.id} s=${s} />`)}</div>
        </details>`)}
      </details>`)}
    </div>
  </nav>`;
}

function Pane({ i }) {
  const m = model.value;
  const lay = m.layout;
  const focus = lay.focus === i;
  const sid = lay.panes[i] ?? (focus ? selected.value : null);
  const s = sid ? m.sessions[sid] : null;
  return html`<section class="pane" data-testid="pane" data-pane=${i} data-focus=${focus} data-state=${s?.state ?? 'empty'}>
    <header class="pane-head"><span class="pane-no">${i + 1}</span>
      ${s ? html`<${Glyph} s=${s} /><span class="pane-title">${s.label}</span>${s.host !== 'local' ? html`<span class="chip">${s.host}</span>` : null}` : html`<span class="muted">empty</span>`}
    </header>
    <div class="pane-body">
      ${s ? html`<dl class="facts">
        <dt>state</dt><dd>${STATE_NAME[s.state]}</dd>
        <dt>folder</dt><dd title=${s.cwd}>${s.cwd}</dd>
        ${s.branch ? html`<dt>branch</dt><dd>${s.branch}</dd>` : null}
        ${s.model ? html`<dt>model</dt><dd>${s.model}</dd>` : null}
        ${s.turns != null ? html`<dt>turns</dt><dd>${s.turns}</dd>` : null}
        ${s.context_tokens != null ? html`<dt>context</dt><dd>${s.context_tokens.toLocaleString()} tokens</dd>` : null}
      </dl>` : html`<div class="pane-empty">
        <div class="coin">PRESS <kbd>N</kbd> TO START</div>
        <div class="hint"><span><kbd>N</kbd> new session</span><span><kbd>T</kbd> terminal</span><span><kbd>K</kbd> palette</span><span><kbd>?</kbd> keys</span></div>
      </div>`}
    </div>
  </section>`;
}

function Board() {
  const lay = model.value.layout;
  return html`<main class="board" data-testid="board" data-grid=${lay.grid}>
    ${Array.from({ length: lay.grid }, (_, i) => html`<${Pane} key=${i} i=${i} />`)}
  </main>`;
}

function Footer() {
  const m = model.value;
  const stats = Object.entries(m.stats);
  return html`<footer class="footer">
    ${stats.map(([h, st]) => html`<span key=${h}>${h} cpu ${Math.round(st.cpu)}% · mem ${st.mem_total ? Math.round((100 * st.mem_used) / st.mem_total) : 0}%</span>`)}
    <span class="push">${serverInfo.value.version ? `claudio ${serverInfo.value.version}` : conn.value}</span>
  </footer>`;
}

export function App() {
  const o = overlay.value;
  return html`<${Topbar} />
    ${banner.value ? html`<div class="banner" role="alert">${banner.value}</div>` : null}
    <div class="main"><${Rail} /><${Board} /></div>
    <${Footer} />
    ${o === 'palette' ? html`<${Palette} />` : o === 'help' ? html`<${Help} />` : o === 'settings' ? html`<${Settings} />` : null}
    <${Modal} /><${Toasts} />`;
}

// ---- focus model: terminal mode forwards everything; app mode owns single keys ----
function isTyping(el) {
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable);
}

function step(delta) {
  const list = orderedSessions(model.value);
  if (!list.length) return;
  const at = list.findIndex((s) => s.id === selected.value);
  selected.value = list[(at + delta + list.length) % list.length].id;
}

export function installFocusModel() {
  defineAction('palette', () => { overlay.value = overlay.value === 'palette' ? null : 'palette'; });
  defineAction('help', () => { overlay.value = 'help'; });
  defineAction('close-overlay', () => { if (modal.value) closeModal(); else overlay.value = null; });
  defineAction('rail-next', () => step(1));
  defineAction('rail-prev', () => step(-1));
  defineAction('next-attention', () => {
    const q = attentionSessions(model.value);
    if (!q.length) return;
    const at = q.findIndex((s) => s.id === selected.value);
    selected.value = q[(at + 1) % q.length].id;
  });
  defineAction('enter-terminal', () => {
    setMode('terminal');
    document.querySelector('[data-testid="terminal"] textarea')?.focus();
  });

  document.addEventListener('keydown', (ev) => {
    if (mode.value === 'terminal') {
      if (isLeaveTerminal(ev)) {
        ev.preventDefault();
        ev.stopPropagation();
        setMode('app');
        document.activeElement?.blur?.();
      }
      return;
    }
    const ctx = { typing: isTyping(document.activeElement), overlay: overlay.value !== null || modal.value !== null };
    const action = appAction(ev, ctx);
    if (!action || ev.repeat) return;
    if (action === 'close-overlay' && !ctx.overlay) return;
    ev.preventDefault();
    runAction(action);
  }, true);

  document.addEventListener('pointerdown', (ev) => {
    const inTerm = ev.target instanceof Element && ev.target.closest('[data-testid="terminal"]');
    setMode(inTerm ? 'terminal' : 'app');
  }, true);
}
