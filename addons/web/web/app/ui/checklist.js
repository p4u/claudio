// An addon's checklist (docs/web-protocol.md §4 "Addons", §5): the review step of a launch and the
// setup of an addon's extension row. Every text is the addon's: text nodes only.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { reqQuiet } from '../store.js';
import { sections, initialOff } from '../addon-view.js';
import { Dialog } from './modal.js';
import { toast } from './toasts.js';

export const checklist = signal(null); // {addon, id, review, starting, finish}
export const checklistOpen = () => checklist.value !== null;
export const cancelChecklist = () => checklist.value?.finish('cancel');

// openChecklist({addon, id, review, starting}) -> Promise<'saved' | 'skip' | 'cancel'>.
// `review`: a launch waits on it (Skip shows); `starting`: the session that starts after it.
export function openChecklist({ addon, id, review = false, starting = null }) {
  return new Promise((resolve) => {
    checklist.value?.finish('cancel');
    const entry = { addon, id, review, starting };
    entry.finish = (v) => {
      if (checklist.value === entry) checklist.value = null;
      resolve(v);
    };
    checklist.value = entry;
  });
}

function Checklist({ entry }) {
  const [state, setState] = useState({ loading: true, c: null, error: null, stale: false });
  const [off, setOff] = useState(new Set());
  const [busy, setBusy] = useState(false);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let dead = false;
    reqQuiet('addons.checklist', { addon: entry.addon, checklist: entry.id })
      .then((c) => {
        if (dead) return;
        setState((p) => ({ loading: false, c, error: null, stale: p.stale }));
        setOff(initialOff(c.items));
      })
      .catch((e) => { if (!dead) setState({ loading: false, c: null, error: e.message, stale: false }); });
    return () => { dead = true; };
  }, [entry.addon, entry.id, nonce]);
  const c = state.c;
  const items = c?.items ?? [];
  const toggle = (id) => setOff((cur) => { const n = new Set(cur); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  async function save() {
    setBusy(true);
    try {
      await reqQuiet('addons.checklist.save', { addon: entry.addon, checklist: entry.id, revision: c.revision, answer: { off: [...off] } });
      entry.finish('saved');
    } catch (e) {
      setBusy(false);
      if (e.message === 'stale') { setState((p) => ({ ...p, stale: true })); setNonce((n) => n + 1); }
      else toast({ text: `${entry.addon}: ${e.message}`, level: 'error' });
    }
  }
  const title = c?.title || `Set up ${entry.id}`;
  return html`<${Dialog} title=${title} testid="checklist" wide onClose=${() => entry.finish('cancel')}>
    ${entry.starting ? html`<p class="setup-starting" data-testid="checklist-starting">Starting ${entry.starting} after this…</p>` : null}
    ${c?.intro ? html`<p class="dialog-body">${c.intro}</p>` : null}
    ${state.stale ? html`<p class="ext-note" role="alert" data-testid="checklist-stale">It changed meanwhile: check it again, then save.</p>` : null}
    <div class="setup-bar">
      <span class="muted">${items.length - off.size} of ${items.length} on</span>
      <span class="setup-bulk">
        <button type="button" class="btn btn-sm" data-testid="checklist-all" onClick=${() => setOff(new Set())}>All</button>
        <button type="button" class="btn btn-sm" data-testid="checklist-none" onClick=${() => setOff(new Set(items.map((i) => i.id)))}>None</button>
      </span>
    </div>
    <div class="setup-items">
      ${state.error ? html`<p class="panel-empty panel-error" role="alert">Could not read it: ${state.error}</p>`
        : state.loading ? html`<p class="panel-empty">Reading…</p>` : null}
      ${sections(items).map((g, k) => html`<section class="setup-sec" key=${`${k}:${g.section ?? ''}`}>
        ${g.section !== null ? html`<h3>${g.section} <span class="count">${g.items.length}</span></h3>` : null}
        ${g.items.map((it) => html`<label class="setup-item" key=${it.id}>
          <input type="checkbox" class="toggle" data-testid="checklist-item" data-id=${it.id} checked=${!off.has(it.id)} onChange=${() => toggle(it.id)} />
          <span class="setup-title">${it.label}</span>${it.detail ? html`<span class="setup-detail">${it.detail}</span>` : null}<code class="setup-id">${it.id}</code>
        </label>`)}
      </section>`)}
    </div>
    <div class="dialog-actions">
      <button type="button" class="btn" data-testid="checklist-cancel" onClick=${() => entry.finish('cancel')}>Cancel</button>
      ${entry.review ? html`<button type="button" class="btn" data-testid="checklist-skip" title="Start without it this time" onClick=${() => entry.finish('skip')}>Skip for now</button>` : null}
      <button type="button" class="btn btn-primary" data-testid="checklist-save" disabled=${busy || state.loading || !c} onClick=${save}>Save</button>
    </div>
  <//>`;
}

export function ChecklistHost() {
  const e = checklist.value;
  return e ? html`<${Checklist} key=${`${e.addon}/${e.id}`} entry=${e} />` : null;
}
