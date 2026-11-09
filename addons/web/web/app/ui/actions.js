// Extension actions: each enabled external extension's actions are palette commands. A result is a
// document (reader-style dialog), a notice, text pasted into the focused claude session, a list or a resume.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, focused, req, reqQuiet, onServerEvent } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog, prompt } from './modal.js';
import { toast } from './toasts.js';
import { Prose } from './reader.js';
import { openConversation } from './search.js';

export const doc = signal(null); // { title, markdown }
export const picker = signal(null); // { title, items }

const focusedClaude = () => {
  const s = model.value.sessions[focused.value];
  return s && s.kind === 'claude' ? s : null;
};

export async function applyResult(r) {
  switch (r?.kind) {
    case 'document':
      doc.value = { title: r.title, markdown: r.markdown };
      overlay.value = 'document';
      break;
    case 'notice':
      toast({ text: r.text, level: 'info' });
      break;
    case 'paste': {
      const s = focusedClaude();
      if (!s) { toast({ text: 'Focus a claude session to paste into.', level: 'warn' }); break; }
      await req('session.paste', { sid: s.id, text: r.text, submit: false }).catch(() => {});
      break;
    }
    case 'list':
      picker.value = { title: r.title, items: r.items ?? [] };
      overlay.value = 'list';
      break;
    case 'resume':
      openConversation(r);
      break;
    default:
      toast({ text: 'The extension answered something this page does not know.', level: 'warn' });
  }
}

export async function runAction(ext, action) {
  let input;
  if (action.input != null) {
    input = await prompt({ title: action.title, placeholder: String(action.input), yes: 'Run', testid: 'action-input' });
    if (input === null) return;
  }
  const sid = focusedClaude()?.id;
  try {
    const { result } = await req('extensions.action', { ext: ext.name, action: action.id, ...(input !== undefined ? { input } : {}), ...(sid ? { sid } : {}) });
    await applyResult(result);
  } catch { /* req toasted */ }
}

let registered = [];
async function refresh() {
  let list;
  try { list = (await reqQuiet('extensions.list')).extensions ?? []; } catch { return; }
  for (const off of registered) off();
  registered = [];
  for (const ext of list) {
    if (!ext.enabled || ext.source === 'builtin' || ext.refused) continue;
    for (const action of ext.actions ?? []) {
      registered.push(register({ id: `ext:${ext.name}:${action.id}`, group: ext.name, title: `${ext.name}: ${action.title}`, run: () => runAction(ext, action) }));
    }
  }
}

export function installActions() {
  onServerEvent((ev) => { if (ev.ev === 'snapshot' || ev.ev === 'extensions_changed') refresh(); });
}

export function DocumentView() {
  const d = doc.value;
  if (!d) return null;
  const close = () => { overlay.value = null; };
  return html`<${Dialog} title=${d.title || 'Document'} testid="document" wide onClose=${close}>
    <div class="doc-scroll" data-testid="reader-answer"><${Prose} markdown=${d.markdown} /></div>
  <//>`;
}

export function ActionList() {
  const p = picker.value;
  if (!p) return null;
  const close = () => { overlay.value = null; };
  return html`<${Dialog} title=${p.title || 'Choose'} testid="action-list" onClose=${close}>
    <ul class="snippet-list">
      ${p.items.length === 0 ? html`<li class="wz-note">Nothing to choose.</li>` : null}
      ${p.items.map((it, i) => html`<li key=${i}><button type="button" class="snippet" data-testid="action-item" onClick=${() => { close(); applyResult(it.result); }}>
        <b class="snippet-name">${it.label}</b>${it.detail ? html`<span class="snippet-text">${it.detail}</span>` : null}</button></li>`)}
    </ul>
  <//>`;
}
