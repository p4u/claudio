// Operations on sessions and on the pane layout, plus the per-pane action buttons.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useRef, useState } from 'preact/hooks';
import { model, selected, focused, req, reqQuiet } from '../store.js';
import { applyEvent } from '../model.js';
import { setGrid, dropOnPane, setFocus, placeBeside } from '../panes.js';
import { termSize, focusTerminal } from '../term.js';
import { confirm, prompt, Dialog } from './modal.js';
import { toast } from './toasts.js';
import { narrow } from './narrow.js';

// true/false: the person chose the board or the panes; null: the board only while no pane holds a session
export const showBoard = signal(null);
export const boardShown = (lay) => showBoard.value ?? !lay.panes.some(Boolean);
// The drawer (reader, changes, history, stats, session) is built by later tasks; panes call openDrawer.
export const drawer = signal(null);
export function openDrawer(tab, sid = focused.value) {
  drawer.value = sid ? { tab, sid } : null;
}

// Sessions that were removed: a layout must not keep pointing at them.
const gone = new Set();
export function trackRemoved(ev) {
  if (ev.ev === 'session_removed') gone.add(ev.sid);
  else if (ev.ev === 'session') gone.delete(ev.session.id);
}

// Save a layout: applied at once, reverted when the server refuses it.
export async function saveLayout(wanted) {
  const prev = model.value.layout;
  const next = { ...wanted, panes: wanted.panes.map((p) => (p && gone.has(p) ? null : p)) };
  if (JSON.stringify(prev) === JSON.stringify(next)) return;
  model.value = applyEvent(model.value, { ev: 'layout', layout: next });
  try {
    await reqQuiet('layout.save', { layout: next });
  } catch (e) {
    if (JSON.stringify(model.value.layout) === JSON.stringify(next)) model.value = applyEvent(model.value, { ev: 'layout', layout: prev });
    toast({ text: `layout: ${e.message}`, level: 'error' });
  }
}

export function setGridSize(n) {
  showBoard.value = false;
  return saveLayout(setGrid(model.value.layout, n));
}

export function focusPane(i) {
  const lay = model.value.layout;
  if (i >= 0 && i < lay.grid && lay.focus !== i) return saveLayout(setFocus(lay, i));
}

// Show a session in the focused pane (or focus the pane that already shows it).
export function openSession(sid) {
  selected.value = sid;
  if (narrow.value) { openDrawer('reader', sid); return; } // no panes here: the reader is what Open can show
  showBoard.value = false;
  const lay = model.value.layout;
  const at = lay.panes.indexOf(sid);
  saveLayout(at >= 0 ? setFocus(lay, at) : dropOnPane(lay, lay.focus, sid));
}

// Open a session in the focused pane and put the keyboard in its terminal (the mode badge follows
// the focus, so TERM shows once xterm has it). Used by the palette, Enter on a card or row.
export function openAndEnter(sid) {
  if (!model.value.sessions[sid]) return;
  openSession(sid);
  if (narrow.value) return;
  let tries = 0;
  const go = () => {
    if (focused.value === sid && focusTerminal(sid)) return;
    if (++tries < 30) setTimeout(go, 50);
  };
  setTimeout(go, 0);
}

export function dropSession(pane, sid) {
  if (!model.value.sessions[sid]) return;
  selected.value = sid;
  showBoard.value = false;
  saveLayout(dropOnPane(model.value.layout, pane, sid));
}

const sizeOf = (sid) => termSize(sid) ?? { rows: 24, cols: 80 };
const isDown = (s) => s.dormant || s.state === 'exited';

export async function renameSession(s) {
  const v = await prompt({ title: 'Rename session', value: s.name ?? s.label, yes: 'Rename', testid: 'rename-input' });
  if (v === null) return;
  const name = v.trim();
  req('session.rename', { sid: s.id, name: name === '' ? null : name }).catch(() => {});
}

// ✕ in the pane header: the session leaves this pane slot and keeps running (and stays in the rail).
export function closePane(i) {
  const lay = model.value.layout;
  if (!(i >= 0 && i < lay.grid) || !lay.panes[i]) return;
  showBoard.value = null;
  return saveLayout({ ...lay, panes: lay.panes.map((p, k) => (k === i ? null : p)) });
}

export async function killSession(s) {
  const ok = await confirm({ title: 'Stop session?', body: `${s.label} on ${s.host} stops now. A conversation can be resumed later from its folder.`, yes: 'Stop', no: 'Keep', danger: true });
  if (ok) req('session.kill', { sid: s.id }).catch(() => {});
}

export const chooser = signal(null); // {s, fresh}

async function respawn(s, fresh, proxy) {
  const args = { sid: s.id, fresh, ...sizeOf(s.id), ...(proxy ? { proxy } : {}) };
  try {
    const { sid } = await reqQuiet('session.respawn', args);
    chooser.value = null;
    if (sid && sid !== s.id) {
      const lay = model.value.layout;
      saveLayout({ ...lay, panes: lay.panes.map((p) => (p === s.id ? sid : p)) });
    }
  } catch (e) {
    if (e.message === 'proxy unknown') chooser.value = { s, fresh };
    else toast({ text: `restart: ${e.message}`, level: 'error' });
  }
}

export async function restartSession(s, fresh) {
  if (!isDown(s)) {
    const ok = await confirm({
      title: fresh ? 'Start a new conversation?' : 'Restart and resume?',
      body: fresh ? `${s.label} stops and starts again with an empty conversation.` : `${s.label} stops and resumes its conversation.`,
      yes: fresh ? 'Start new' : 'Restart', no: 'Cancel',
    });
    if (!ok) return;
  }
  return respawn(s, fresh, null);
}

export async function resumeSession(s) {
  return respawn(s, false, null);
}

export async function openTerminalTab(s) {
  try {
    const { sid } = await req('session.shell', { host: s.host, cwd: s.cwd, ...sizeOf(s.id) });
    showBoard.value = false;
    saveLayout(placeBeside(model.value.layout, s.id, sid));
  } catch { /* req already told the person */ }
}

function ProxyChooser() {
  const c = chooser.value;
  const [choice, setChoice] = useState('default');
  if (!c) return null;
  const names = model.value.profiles.names;
  const def = model.value.profiles.default;
  const go = () => {
    const proxy = choice === 'default' ? { kind: 'default' } : choice === 'direct' ? { kind: 'direct' } : { kind: 'profile', name: choice.slice(8) };
    respawn(c.s, c.fresh, proxy);
  };
  return html`<${Dialog} title="Which proxy?" testid="proxy-chooser" onClose=${() => { chooser.value = null; }}>
    <p class="dialog-body">claudio does not know how ${c.s.label} was started. Pick the proxy it should use.</p>
    <select class="select" data-testid="proxy-choice" data-autofocus value=${choice} onChange=${(e) => setChoice(e.target.value)}>
      <option value="default" selected=${choice === 'default'}>${def ? `Default (${def})` : 'Default (direct)'}</option>
      <option value="direct" selected=${choice === 'direct'}>Direct, no proxy</option>
      ${names.map((n) => html`<option value=${`profile:${n}`} selected=${choice === `profile:${n}`}>${n}</option>`)}
    </select>
    <div class="dialog-actions">
      <button class="btn" data-testid="confirm-no" onClick=${() => { chooser.value = null; }}>Cancel</button>
      <button class="btn btn-primary" data-testid="confirm-yes" onClick=${go}>Restart</button>
    </div>
  <//>`;
}

export function ProxyChooserHost() {
  return chooser.value ? html`<${ProxyChooser} />` : null;
}

// The pane header's actions. Wide: icon buttons. In a narrow pane (container query in layout.css)
// they fold into one `⋯` button that opens the same buttons as a labelled menu, so every
// data-testid stays on its real button; open the menu first (`[data-testid="session-menu-btn"]`).
export function SessionActions({ s, pane = null }) {
  const shell = s.kind === 'shell';
  const [open, setOpen] = useState(false);
  const root = useRef(null);
  useEffect(() => {
    if (!open) return undefined;
    const away = (e) => { if (!root.current?.contains(e.target)) setOpen(false); };
    const esc = (e) => { if (e.key === 'Escape') { e.stopPropagation(); setOpen(false); root.current?.querySelector('[data-testid="session-menu-btn"]')?.focus(); } };
    document.addEventListener('pointerdown', away, true);
    document.addEventListener('keydown', esc, true);
    return () => { document.removeEventListener('pointerdown', away, true); document.removeEventListener('keydown', esc, true); };
  }, [open]);
  const run = (fn) => () => { setOpen(false); fn(); };
  const item = (testid, ico, label, tip, fn, cls = '') => html`<button type="button" class=${`icon-btn act ${cls}`} data-testid=${testid} title=${tip} aria-label=${tip} onClick=${run(fn)}><span class="act-ico" aria-hidden="true">${ico}</span><span class="act-label">${label}</span></button>`;
  return html`<div class="session-actions" ref=${root} data-testid="session-actions" data-open=${open}>
    <button type="button" class="icon-btn more-btn" data-testid="session-menu-btn" aria-haspopup="menu" aria-expanded=${open} aria-label="Session actions" title="Session actions" onClick=${() => setOpen(!open)}>⋯</button>
    <div class="act-row" role=${open ? 'menu' : null}>
      ${item('rename-btn', '✎', 'Rename', 'Rename', () => renameSession(s))}
      ${shell ? null : html`
        ${item('restart-resume-btn', '↻', 'Restart (resume)', isDown(s) ? 'Resume the conversation' : 'Restart and resume the conversation', () => (isDown(s) ? resumeSession(s) : restartSession(s, false)))}
        ${item('restart-new-btn', '✦', 'New conversation', 'Restart with a new conversation', () => restartSession(s, true))}
        ${item('terminal-tab-btn', '$_', 'Terminal beside', 'Open a terminal in this folder beside this pane', () => openTerminalTab(s))}`}
      <span class="act-sep" role="separator"></span>
      ${item('kill-btn', '⏻', 'Stop session', 'Stop this session…', () => killSession(s), 'danger')}
      ${pane === null ? null : html`<span class="act-sep act-gap" role="separator"></span>${item('close-pane-btn', '✕', 'Close pane', 'Close this pane (the session keeps running)', () => closePane(pane))}`}
    </div>
  </div>`;
}
