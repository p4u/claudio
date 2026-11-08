import { html } from 'htm/preact';
import { effect } from '@preact/signals';
import { model, mode, setMode, banner, selected, focused, conn, serverInfo, onServerEvent } from '../store.js';
import { orderedSessions } from '../model.js';
import { appAction, isLeaveTerminal, tuiBinding, describeKey } from '../keymap.js';
import { overlay, defineAction, runAction } from '../palette.js';
import { settings, updateSettings } from '../settings.js';
import { planLayout, stageFloor } from '../layout.js';
import { viewportW } from './viewport.js';
import { attentionQueue } from '../queue.js';
import { makeNotifier, titleText } from '../notify.js';
import { focusTerminal } from '../term.js';
import { Topbar } from './topbar.js';
import { ToastList } from './toasts.js';
import { Modal, modal, closeModal } from './modal.js';
import { Palette } from './palette.js';
import { Help } from './help.js';
import { Settings } from './settings.js';
import { Glyph, glyphFor, ago } from './glyph.js';
import { Rail } from './rail.js';
import { Board } from './board.js';
import { PaneGrid } from './pane.js';
import { queueSessions } from './approve.js';
import { InfoPopover } from './info.js';
import { NarrowView, narrow } from './narrow.js';
import { Wizard, installWizard, openWizard } from './wizard.js';
import { Search, installSearch } from './search.js';
import { Snippets, installSnippets } from './snippets.js';
import { ComposeDialog, installCompose } from './composer.js';
import { installMaximize } from './maximize.js';
import { DocumentView, ActionList, installActions } from './actions.js';
import { Drawer } from './drawer.js';
import './reader.js';
import './changes.js';
import './history.js';
import './stats.js';
import './session-tab.js';
import { ExtensionsScreen, installExtensions } from './extensions.js';
import { PluginSetupHost, installPluginGate, setupOpen, cancelPluginSetup } from './plugin-setup.js';
import { ClaudeToastList } from './claude.js';
import { HostPanel, installHosts } from './hosts.js';
import { drawer, showBoard, boardShown, setGridSize, focusPane, openSession, openAndEnter, trackRemoved, ProxyChooserHost, renameSession, killSession, restartSession, openTerminalTab } from './session-actions.js';
import { keyCheck, keyCheckLog } from './key-check.js';
import './capture.js';

export { glyphFor, Glyph, ago };

// Sessions waiting for the person (approval or input), approval first.
export function attentionSessions(m) {
  return attentionQueue(Object.values(m.sessions), Date.now()).filter((s) => s.state === 'needs_approval' || s.state === 'needs_input');
}

function StageBar({ board, railCollapsed, railAuto }) {
  const lay = model.value.layout;
  return html`<div class="stage-bar">
    <button class="btn btn-sm btn-icon" data-testid="rail-toggle" aria-pressed=${railCollapsed} aria-label=${railCollapsed ? 'Show the session list' : 'Hide the session list'}
      title=${railAuto ? 'The window is too narrow for the list beside the drawer. Close the drawer or widen the window.' : railCollapsed ? 'Show the session list' : 'Collapse the session list to icons'}
      onClick=${() => updateSettings({ railCollapsed: !settings.value.railCollapsed })}>${railCollapsed ? '»' : '«'}</button>
    <button class="btn btn-sm" data-testid="new-session-btn" title="New session (n)" onClick=${() => openWizard({ kind: 'claude' })}>+ New</button>
    <span class="stage-sep"></span>
    <button class="btn btn-sm" data-testid="board-btn" aria-pressed=${board} title="Show every session as a row" onClick=${() => { showBoard.value = !board; }}>Board</button>
    <span class="stage-sep"></span>
    <span class="stage-label">Panes</span>
    ${[1, 2, 4].map((n) => html`<button class="btn btn-sm" data-testid="grid-btn" data-grid=${n} aria-pressed=${!board && lay.grid === n} aria-label=${`${n} pane${n > 1 ? 's' : ''}`} title=${n === 1 ? 'One pane' : n === 2 ? 'Two panes side by side' : 'Four panes in a grid'} onClick=${() => setGridSize(n)}>${n}</button>`)}
  </div>`;
}

function Stage({ railCollapsed, railAuto }) {
  const board = boardShown(model.value.layout);
  return html`<section class="stage" data-testid="stage" data-view=${board ? 'board' : 'panes'}>
    <${StageBar} board=${board} railCollapsed=${railCollapsed} railAuto=${railAuto} />
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
  const lay = model.value.layout;
  const grid = boardShown(lay) ? 1 : lay.grid;
  const plan = planLayout({
    vw: viewportW.value, grid, drawerOpen: !!drawer.value && !small,
    drawerW: settings.value.drawerWidth, railManual: settings.value.railCollapsed,
  });
  const style = small ? null : { '--rail-w': `${plan.railW}px`, '--drawer-w': `${plan.drawerW}px`, '--stage-floor': `${stageFloor(grid)}px` };
  return html`<${Topbar} />
    ${banner.value ? html`<div class="banner" role="alert">${banner.value}</div>` : null}
    <div class="main" style=${style} data-narrow=${small} data-drawer=${!!drawer.value} data-overlay=${plan.overlay} data-rail=${plan.railCollapsed ? 'strip' : 'full'} data-drawer-tab=${drawer.value?.tab ?? ''}>${small ? html`<${NarrowView} Board=${Board} />` : html`<${Rail} strip=${plan.railCollapsed} /><${Stage} railCollapsed=${plan.railCollapsed} railAuto=${plan.railAuto} />`}<${Drawer} plan=${plan} /></div>
    <${Footer} />
    ${o === 'palette' ? html`<${Palette} />` : o === 'help' ? html`<${Help} />` : o === 'settings' ? html`<${Settings} />`
      : o === 'wizard' ? html`<${Wizard} />` : o === 'search' ? html`<${Search} />` : o === 'snippets' ? html`<${Snippets} />` : o === 'compose' ? html`<${ComposeDialog} />`
      : o === 'document' ? html`<${DocumentView} />` : o === 'list' ? html`<${ActionList} />`
      : o === 'extensions' ? html`<${ExtensionsScreen} />` : o === 'hosts' ? html`<${HostPanel} />` : null}
    <${PluginSetupHost} /><${ProxyChooserHost} /><${Modal} />
    <${InfoPopover} />
    <div class="toasts" aria-live="polite"><${ClaudeToastList} /><${ToastList} /></div>`;
}

// ---- focus model: terminal mode forwards everything; app mode owns single keys ----
function isTyping(el) {
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable);
}

// Rail arrows: the selection ring moves at once, the focused pane follows once the person pauses.
let followTimer = null;
function step(delta) {
  const list = orderedSessions(model.value);
  if (!list.length) return;
  const at = list.findIndex((s) => s.id === selected.value);
  const sid = list[(at + delta + list.length) % list.length].id;
  selected.value = sid;
  setTimeout(() => document.querySelector(`[data-sid="${CSS.escape(sid)}"]`)?.scrollIntoView({ block: 'nearest' }), 0);
  clearTimeout(followTimer);
  followTimer = setTimeout(() => { if (selected.value === sid && model.value.sessions[sid]) openSession(sid); }, 160);
}

export function installFocusModel() {
  defineAction('palette', () => { overlay.value = overlay.value === 'palette' ? null : 'palette'; });
  defineAction('help', () => { overlay.value = 'help'; });
  defineAction('close-overlay', () => { if (modal.value) closeModal(); else if (setupOpen()) cancelPluginSetup(); else overlay.value = null; });
  defineAction('rail-next', () => step(1));
  defineAction('rail-prev', () => step(-1));
  // `a`: the first card of the queue, in the focused pane
  defineAction('next-attention', () => {
    const q = queueSessions();
    if (q.length) openSession(q[0].id);
  });
  // TUI parity (keymap.js TUI_BINDINGS): relative moves and jumps in rail order, in the focused pane
  const keep = (sid) => (mode.value === 'terminal' ? openAndEnter(sid) : openSession(sid));
  const move = (delta) => {
    const list = orderedSessions(model.value);
    if (!list.length) return;
    const at = list.findIndex((x) => x.id === focused.value);
    keep(list[at < 0 ? (delta > 0 ? 0 : list.length - 1) : (at + delta + list.length) % list.length].id);
  };
  defineAction('session-next', () => move(1));
  defineAction('session-prev', () => move(-1));
  for (let n = 1; n <= 10; n++) {
    defineAction(`goto-session-${n}`, () => { const x = orderedSessions(model.value)[n - 1]; if (x) keep(x.id); });
  }
  const withFocused = (fn) => () => { const x = model.value.sessions[focused.value]; if (x) fn(x); };
  defineAction('terminal-beside', withFocused(openTerminalTab));
  defineAction('rename-focused', withFocused(renameSession));
  defineAction('kill-focused', withFocused(killSession));
  defineAction('restart-focused', withFocused((x) => restartSession(x, false)));
  defineAction('board', () => { showBoard.value = !boardShown(model.value.layout); });
  defineAction('extensions', () => { overlay.value = 'extensions'; });
  for (let n = 1; n <= 4; n++) {
    defineAction(`focus-pane-${n}`, () => {
      if (narrow.value || n > model.value.layout.grid) return;
      showBoard.value = false;
      focusPane(n - 1);
    });
  }
  defineAction('enter-terminal', () => {
    clearTimeout(followTimer);
    const want = selected.value;
    if (want && want !== focused.value && model.value.sessions[want]) { openAndEnter(want); return; }
    if (focused.value) focusTerminal(focused.value); // the mode badge follows the focus
  });
  installWizard();
  installSearch();
  installSnippets();
  installCompose();
  installMaximize();
  installActions();
  installExtensions();
  installHosts();
  installPluginGate();
  onServerEvent(trackRemoved);
  installNotifications();

  document.addEventListener('keydown', (ev) => {
    // The help's key check only reports: nothing runs and nothing reaches a session (Esc closes the help).
    if (keyCheck.value && overlay.value === 'help' && ev.key !== 'Escape') {
      ev.preventDefault();
      ev.stopPropagation();
      keyCheckLog.value = describeKey(ev);
      return;
    }
    // TUI parity keys: before xterm and the browser see them, in a terminal or out of it.
    const tui = modal.value ? null : tuiBinding(ev);
    if (tui) {
      ev.preventDefault();
      ev.stopPropagation();
      if (!ev.repeat) runAction(tui);
      return;
    }
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
    const ctx = { typing: isTyping(document.activeElement), overlay: overlay.value !== null || modal.value !== null || setupOpen() };
    const action = appAction(ev, ctx);
    if (!action || ev.repeat) return;
    if (action === 'close-overlay' && !ctx.overlay) return;
    ev.preventDefault();
    runAction(action);
  }, true);

  // The mode is where the focus is: inside a terminal host it is TERM, anywhere else APP.
  const inTerm = (el) => el instanceof Element && !!el.closest('[data-testid="terminal"]');
  document.addEventListener('focusin', (ev) => setMode(inTerm(ev.target) ? 'terminal' : 'app'), true);
  document.addEventListener('focusout', (ev) => {
    if (ev.relatedTarget) return; // focusin of the next element decides
    setTimeout(() => { if (document.hasFocus() && !inTerm(document.activeElement)) setMode('app'); }, 0);
  }, true);
  setMode(inTerm(document.activeElement) ? 'terminal' : 'app');
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
