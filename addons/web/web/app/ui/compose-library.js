// The side panel of the Compose dialog: "Saved" (the named prompt library, claudio's snippets: global,
// shared with the TUI) and "Recent" (snapshots of drafts, kept in this browser).
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { req, reqQuiet } from '../store.js';
import { tooLarge } from '../compose.js';
import { slug, firstLine, filterItems, nameFromText, hasName } from '../library.js';
import { confirm, prompt } from './modal.js';
import { toast } from './toasts.js';
import { drafts } from './drafts.js';

// The library: null until loaded; {name, text}[] sorted by name.
export const library = signal({ items: null, err: null });

export function loadLibrary() {
  return reqQuiet('snippets.list')
    .then((r) => { library.value = { items: r.snippets ?? [], err: null }; })
    .catch((e) => { library.value = { items: library.value.items, err: e.message }; });
}

// Ask for a name and save `text` under it; asks before replacing a saved prompt of that name.
export async function saveAs(text, suggested = nameFromText(text)) {
  if (!String(text).trim()) { toast({ text: 'There is nothing to save.', level: 'warn' }); return false; }
  if (tooLarge(text)) { toast({ text: 'That is over 64 KiB. It cannot be saved.', level: 'warn' }); return false; }
  const name = await prompt({ title: 'Save prompt as', value: suggested, yes: 'Save', placeholder: 'a name', testid: 'save-as-input' });
  if (name === null || !name.trim()) return false;
  return put(name, text);
}

async function put(name, text) {
  if (library.value.items === null) await loadLibrary();
  const old = hasName(library.value.items, name);
  if (old && old.text !== text) {
    const ok = await confirm({ title: 'Replace the saved prompt?', body: `"${old.name}" is already saved. Its text is replaced.`, yes: 'Replace', no: 'Keep', danger: true });
    if (!ok) return false;
  }
  try {
    const { name: saved } = await req('snippets.save', { text, name });
    toast({ text: `Saved as ${saved}.`, level: 'info' });
    await loadLibrary();
    return true;
  } catch { return false; }
}

async function rename(item) {
  const name = await prompt({ title: `Rename ${item.name}`, value: item.name, yes: 'Rename', placeholder: 'a name', testid: 'rename-prompt-input' });
  if (name === null || !name.trim() || slug(name) === item.name) return;
  if (!(await put(name, item.text))) return; // saved under the new name first, so a failure loses nothing
  try { await req('snippets.delete', { name: item.name }); } catch { /* req told the person */ }
  await loadLibrary();
}

async function remove(item) {
  const ok = await confirm({ title: 'Delete the saved prompt?', body: `"${item.name}" is deleted from the library, for every session.`, yes: 'Delete', no: 'Keep', danger: true });
  if (!ok) return;
  try { await req('snippets.delete', { name: item.name }); } catch { return; }
  await loadLibrary();
}

const ageOf = (at, nowMs) => {
  const d = Math.max(0, Math.floor((nowMs - at) / 1000));
  return d < 60 ? `${d}s ago` : d < 3600 ? `${Math.floor(d / 60)}m ago` : d < 86400 ? `${Math.floor(d / 3600)}h ago` : `${Math.floor(d / 86400)}d ago`;
};

function Saved({ onReplace, onAppend }) {
  const [q, setQ] = useState('');
  const { items, err } = library.value;
  useEffect(() => { loadLibrary(); }, []);
  const shown = filterItems(items, q);
  return html`<div class="lib-pane" data-testid="lib-saved">
    <input class="input" data-testid="lib-filter" placeholder="Filter" aria-label="Filter saved prompts" autocomplete="off" spellcheck=${false} value=${q} onInput=${(e) => setQ(e.target.value)} />
    ${err ? html`<p class="wz-warn" role="alert">${err}</p>` : null}
    <ul class="lib-list">
      ${items === null && !err ? html`<li class="wz-note">Loading…</li>` : null}
      ${items?.length === 0 ? html`<li class="wz-note">Nothing saved yet. Write a prompt and press Save as…</li>` : null}
      ${items?.length && !shown.length ? html`<li class="wz-note">No match.</li>` : null}
      ${shown.map((it) => html`<li class="lib-item" key=${it.name} data-testid="lib-item" data-name=${it.name}>
        <b class="lib-name">${it.name}</b>
        <span class="lib-line">${firstLine(it.text)}</span>
        <span class="lib-actions">
          <button type="button" class="btn btn-sm" data-testid="lib-load" title="Replace the draft with this prompt (the draft is kept in Recent)" onClick=${() => onReplace(it.text)}>Load</button>
          <button type="button" class="btn btn-sm" data-testid="lib-append" title="Add this prompt after the draft" onClick=${() => onAppend(it.text)}>Append</button>
          <button type="button" class="btn btn-sm" data-testid="lib-rename" onClick=${() => rename(it)}>Rename</button>
          <button type="button" class="btn btn-sm" data-testid="lib-delete" onClick=${() => remove(it)}>Delete</button>
        </span>
      </li>`)}
    </ul>
  </div>`;
}

function Recent({ now, onReplace, bump }) {
  const list = drafts.recent();
  return html`<div class="lib-pane" data-testid="lib-recent">
    <ul class="lib-list">
      ${list.length === 0 ? html`<li class="wz-note">Prompts you send, insert, clear or replace show up here, so none is lost.</li>` : null}
      ${list.map((r) => html`<li class="lib-item lib-recent" key=${r.id} data-testid="recent-item">
        <button type="button" class="lib-open" data-testid="recent-load" title="Load it into the editor (the draft is kept in Recent)" onClick=${() => onReplace(r.text)}>
          <span class="lib-name">${r.label || 'session'} · ${ageOf(r.at, now)}</span>
          <span class="lib-line">${firstLine(r.text)}</span>
        </button>
        <button type="button" class="icon-btn lib-x" data-testid="recent-remove" aria-label="Remove from Recent" title="Remove from Recent" onClick=${() => { drafts.remove(r.id); bump(); }}>×</button>
      </li>`)}
    </ul>
  </div>`;
}

export function ComposeSide({ sid, text, now, onReplace, onAppend }) {
  const [tab, setTab] = useState('saved');
  const [, setN] = useState(0);
  const bump = () => setN((n) => n + 1);
  const recentCount = drafts.recent().length;
  const replace = (t) => { onReplace(t); bump(); };
  return html`<aside class="compose-side" aria-label="Prompt library" data-sid=${sid}>
    <div class="host-tabs" role="tablist">
      <button type="button" class="seg-btn" role="tab" data-testid="lib-tab-saved" aria-selected=${tab === 'saved'} onClick=${() => setTab('saved')}>Saved ${library.value.items ? html`<span class="count">${library.value.items.length}</span>` : null}</button>
      <button type="button" class="seg-btn" role="tab" data-testid="lib-tab-recent" aria-selected=${tab === 'recent'} onClick=${() => { setTab('recent'); bump(); }}>Recent <span class="count">${recentCount}</span></button>
    </div>
    ${tab === 'saved' ? html`<${Saved} onReplace=${replace} onAppend=${onAppend} />` : html`<${Recent} now=${now} onReplace=${replace} bump=${bump} />`}
  </aside>`;
}
