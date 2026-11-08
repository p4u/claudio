import { html } from 'htm/preact';
import { effect } from '@preact/signals';
import { model, mode, setMode, banner, selected, focused, conn, serverInfo, onServerEvent } from '../store.js';
import { orderedSessions } from '../model.js';
import { appAction, isLeaveTerminal } from '../keymap.js';
import { overlay, defineAction, runAction } from '../palette.js';
import { settings } from '../settings.js';
import { attentionQueue } from '../queue.js';
import { makeNotifier, titleText } from '../notify.js';
import { focusTerminal } from '../term.js';
import { Topbar } from './topbar.js';
import { Toasts } from './toasts.js';
import { Modal, modal, closeModal } from './modal.js';
import { Palette } from './palette.js';
import { Help } from './help.js';
import { Settings } from './settings.js';
import { Glyph, glyphFor, ago } from './glyph.js';
import { Rail } from './rail.js';
import { Board } from './board.js';
import { PaneGrid } from './pane.js';
import { Queue, queueSessions } from './queue.js';
import { NarrowView, narrow } from './narrow.js';
import { Wizard, installWizard, openWizard } from './wizard.js';
import { Search, installSearch } from './search.js';
import { Snippets, installSnippets } from './snippets.js';
import { DocumentView, ActionList, installActions } from './actions.js';
import { Drawer } from './drawer.js';
import './reader.js';
import { drawer, showBoard, boardShown, setGridSize, focusPane, openSession, trackRemoved, ProxyChooserHost } from './session-actions.js';

export { glyphFor, Glyph, ago };

// Sessions waiting for the person (approval or input), approval first.
export function attentionSessions(m) {
  return attentionQueue(Object.values(m.sessions), Date.now()).filter((s) => s.state === 'needs_approval' || s.state === 'needs_input');
}

function StageBar({ board }) {
  const lay = model.value.layout;
  return html`<div class="stage-bar">
    <button class="btn btn-sm btn-primary" data-testid="new-session-btn" title="New session (n)" onClick=${() => openWizard({ kind: 'claude' })}>+ New</button>
    <span class="stage-sep"></span>
    <button class="btn btn-sm" data-testid="board-btn" aria-pressed=${board} title="Show every session as a row" onClick=${() => { showBoard.value = !board; }}>Board</button>
    <span class="stage-sep"></span>
    <span class="stage-label">Panes</span>
    ${[1, 2, 4].map((n) => html`<button class="btn btn-sm" data-testid="grid-btn" data-grid=${n} aria-pressed=${!board && lay.grid === n} aria-label=${`${n} pane${n > 1 ? 's' : ''}`} title=${n === 1 ? 'One pane' : n === 2 ? 'Two panes side by side' : 'Four panes in a grid'} onClick=${() => setGridSize(n)}>${n}</button>`)}
  </div>`;
}

function Stage() {
  const board = boardShown(model.value.layout);
  return html`<section class="stage" data-testid="stage" data-view=${board ? 'board' : 'panes'}>
    <${StageBar} board=${board} />
    ${board ? html`<${Board} />` : html`<${PaneGrid} />`}
  </section>`;
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
  const small = narrow.value;
  return html`<${Topbar} />
    ${banner.value ? html`<div class="banner" role="alert">${banner.value}</div>` : null}
    <div class="main" data-narrow=${small} data-drawer=${!!drawer.value}>${small ? html`<${NarrowView} Queue=${Queue} Board=${Board} />` : html`<${Rail} /><${Stage} />`}<${Drawer} /></div>
    <${Footer} />
    ${o === 'palette' ? html`<${Palette} />` : o === 'help' ? html`<${Help} />` : o === 'settings' ? html`<${Settings} />`
      : o === 'wizard' ? html`<${Wizard} />` : o === 'search' ? html`<${Search} />` : o === 'snippets' ? html`<${Snippets} />`
      : o === 'document' ? html`<${DocumentView} />` : o === 'list' ? html`<${ActionList} />` : null}
    <${ProxyChooserHost} /><${Modal} /><${Toasts} />`;
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
  // `a`: the first card of the queue, in the focused pane
  defineAction('next-attention', () => {
    const q = queueSessions();
    if (q.length) openSession(q[0].id);
  });
  for (let n = 1; n <= 4; n++) {
    defineAction(`focus-pane-${n}`, () => {
      if (narrow.value || n > model.value.layout.grid) return;
      showBoard.value = false;
      focusPane(n - 1);
    });
  }
  defineAction('enter-terminal', () => {
    const want = selected.value;
    if (want && want !== focused.value && model.value.sessions[want]) {
      openSession(want);
      setTimeout(() => { if (focusTerminal(want)) setMode('terminal'); }, 150);
      return;
    }
    if (focused.value && focusTerminal(focused.value)) setMode('terminal');
  });
  installWizard();
  installSearch();
  installSnippets();
  installActions();
  onServerEvent(trackRemoved);
  installNotifications();

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
    // Enter on a focused button, row or summary activates it; it is not "type into the terminal".
    if (ev.key === 'Enter' && ev.target instanceof Element && ev.target.closest('button, a, summary, [role="button"], [role="row"]')) return;
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

// ---- notifications: the tab title always, the desktop notification when the person turned it on ----
function blip() {
  try {
    const ctx = new (globalThis.AudioContext || globalThis.webkitAudioContext)();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = 'square';
    osc.frequency.value = 880;
    gain.gain.value = 0.04;
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start();
    osc.stop(ctx.currentTime + 0.09);
    osc.onended = () => ctx.close();
  } catch { /* no audio */ }
}

function installNotifications() {
  const notifier = makeNotifier();
  let primed = false;
  const run = () => {
    const m = model.value;
    const s = settings.value;
    const sessions = Object.values(m.sessions);
    const shown = document.visibilityState === 'visible' && !narrow.value ? m.layout.panes.filter(Boolean) : [];
    document.title = titleText(sessions.filter((x) => x.kind !== 'shell' && (x.state === 'needs_approval' || x.state === 'needs_input' || x.state === 'error')).length);
    if (!primed) { // the first snapshot is what the person is looking at: arm, don't announce
      if (Object.keys(m.hosts).length === 0) return;
      primed = true;
      notifier.check(sessions, shown);
      return;
    }
    for (const hit of notifier.check(sessions, shown)) {
      if (s.sound) blip();
      if (s.notifications && globalThis.Notification?.permission === 'granted') {
        const n = new Notification(hit.title, { body: hit.body, tag: hit.sid });
        n.onclick = () => { window.focus(); openSession(hit.sid); n.close(); };
      }
    }
  };
  effect(run);
  document.addEventListener('visibilitychange', run);
}
