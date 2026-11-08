// Prompt snippets: inserted into the Compose editor of the focused claude session (never sent), saved from its draft or the last prompt.
import { html } from 'htm/preact';
import { useEffect, useState } from 'preact/hooks';
import { model, focused, req, reqQuiet } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog } from './modal.js';
import { toast } from './toasts.js';
import { insertIntoComposer, focusedDraft } from './composer.js';

export function openSnippets() {
  overlay.value = 'snippets';
}

export async function saveSnippet(text) {
  const t = String(text ?? '');
  if (!t.trim()) { toast({ text: 'There is nothing to save.', level: 'warn' }); return false; }
  try {
    const { name } = await req('snippets.save', { text: t });
    toast({ text: `Saved the snippet as ${name}.`, level: 'info' });
    return true;
  } catch { return false; }
}

export async function saveLastPrompt() {
  const sid = focused.value;
  if (!sid || model.value.sessions[sid]?.kind !== 'claude') { toast({ text: 'Focus a claude session first.', level: 'warn' }); return; }
  try {
    const { prompt, cut } = await req('reader.last_prompt', { sid });
    if (prompt == null) { toast({ text: 'That session has no prompt yet.', level: 'warn' }); return; }
    if (await saveSnippet(prompt) && cut) toast({ text: 'The prompt was long; the saved snippet is cut.', level: 'warn' });
  } catch { /* req toasted */ }
}

export function Snippets() {
  const [items, setItems] = useState(null);
  const [err, setErr] = useState(null);
  const load = () => reqQuiet('snippets.list').then((r) => setItems(r.snippets ?? [])).catch((e) => setErr(e.message));
  useEffect(() => { load(); }, []);
  const close = () => { overlay.value = null; };
  const insert = (sn) => {
    if (!insertIntoComposer(sn.text)) close(); // opens Compose, which replaces this list
  };
  const composerText = focusedDraft();
  return html`<${Dialog} title="Snippets" testid="snippets" wide onClose=${close}>
    <p class="dialog-body">Click a snippet to put it in the Compose editor. It is never sent for you.</p>
    ${err ? html`<p class="wz-warn" role="alert">${err}</p>` : null}
    <ul class="snippet-list">
      ${items === null && !err ? html`<li class="wz-note">Loading…</li>` : null}
      ${items?.length === 0 ? html`<li class="wz-note">No snippets yet. Write a prompt, then save it here.</li>` : null}
      ${(items ?? []).map((sn) => html`<li key=${sn.name}><button type="button" class="snippet" data-testid="snippet" onClick=${() => insert(sn)}>
        <b class="snippet-name">${sn.name}</b><span class="snippet-text">${sn.text}</span></button></li>`)}
    </ul>
    <div class="dialog-actions">
      <button type="button" class="btn" data-testid="snippet-save" disabled=${!composerText.trim()} title="Save the draft in the Compose editor"
        onClick=${async () => { if (await saveSnippet(composerText)) load(); }}>Save the Compose draft</button>
      <button type="button" class="btn" onClick=${close}>Close</button>
    </div>
  <//>`;
}

export function installSnippets() {
  register({ id: 'snippets:insert', group: 'Prompt', title: 'Insert snippet', run: openSnippets });
  register({ id: 'snippets:save-last', group: 'Prompt', title: 'Save last prompt as snippet', run: saveLastPrompt });
}
