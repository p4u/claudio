// Operations on sessions and on the pane layout, plus the per-pane action buttons.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useState } from 'preact/hooks';
import { model, selected, focused, req, reqQuiet } from '../store.js';
import { applyEvent } from '../model.js';
import { setGrid, dropOnPane, setFocus, placeBeside } from '../panes.js';
import { termSize } from '../term.js';
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
  if (narrow.value) return;
  showBoard.value = false;
  const lay = model.value.layout;
  const at = lay.panes.indexOf(sid);
  saveLayout(at >= 0 ? setFocus(lay, at) : dropOnPane(lay, lay.focus, sid));
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

export async function killSession(s) {
  const ok = await confirm({ title: 'Kill session?', body: `${s.label} on ${s.host} stops now. A conversation can be resumed later from its folder.`, yes: 'Kill', no: 'Keep' });
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

export function SessionActions({ s }) {
  const shell = s.kind === 'shell';
  return html`<div class="session-actions" data-testid="session-actions">
    <button class="icon-btn" data-testid="rename-btn" title="Rename" aria-label="Rename session" onClick=${() => renameSession(s)}>✎</button>
    ${shell ? null : html`
      <button class="icon-btn" data-testid="restart-resume-btn" title=${isDown(s) ? 'Resume the conversation' : 'Restart and resume the conversation'} aria-label=${isDown(s) ? 'Resume' : 'Restart and resume'} onClick=${() => (isDown(s) ? resumeSession(s) : restartSession(s, false))}>↻</button>
      <button class="icon-btn" data-testid="restart-new-btn" title="Restart with a new conversation" aria-label="Restart with a new conversation" onClick=${() => restartSession(s, true)}>✦</button>
      <button class="icon-btn" data-testid="terminal-tab-btn" title="Open a terminal in this folder" aria-label="Open a terminal tab" onClick=${() => openTerminalTab(s)}>$_</button>`}
    <button class="icon-btn danger" data-testid="kill-btn" title="Kill" aria-label="Kill session" onClick=${() => killSession(s)}>✕</button>
  </div>`;
}
