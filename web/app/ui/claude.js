// Claude install/update offers and progress, as non-modal toasts. They never take focus and
// handle no keys: the person reaches their buttons with the mouse or Tab, like any other control.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { model, req } from '../store.js';

const dismissed = signal({}); // host -> key of the claude event the person closed

const keyOf = (c) => `${c.phase}|${c.have ?? ''}|${c.want ?? ''}|${c.version ?? ''}|${c.tail ?? ''}`;
// An update offer is not a toast: it is the `↑` on the host chip (see updateOffered). Toasts are for
// an install offer on a host with no claude, progress, done and failure.
export const updateOffered = (c) => !!c && c.phase === 'offer_update';
const shown = (c) => c.phase !== 'offer_update' && dismissed.value[c.host] !== keyOf(c);
const close = (c) => { dismissed.value = { ...dismissed.value, [c.host]: keyOf(c) }; };

async function act(c, install) {
  try { await req('claude.update', { host: c.host, install }); } catch { /* req toasted */ }
}
async function skip(c) {
  close(c);
  try { await req('claude.skip', { host: c.host }); } catch { /* req toasted */ }
}

export function claudeText(c) {
  switch (c.phase) {
    case 'offer_install': return `Install claude on ${c.host}?`;
    case 'offer_update': return `claude ${c.have ?? '?'} → ${c.want ?? '?'} on ${c.host}`;
    case 'installing': return `Installing claude on ${c.host}…`;
    case 'updating': return `Updating claude on ${c.host}…`;
    case 'done': return `claude ${c.version ?? ''} on ${c.host}`.replace('  ', ' ');
    case 'failed': return `claude failed on ${c.host}`;
    default: return `claude on ${c.host}`;
  }
}

function ClaudeToast({ c }) {
  const offer = c.phase === 'offer_install' || c.phase === 'offer_update';
  const busy = c.phase === 'installing' || c.phase === 'updating';
  useEffect(() => {
    if (c.phase !== 'done') return undefined;
    const t = setTimeout(() => close(c), 8000);
    return () => clearTimeout(t);
  }, [c.phase, c.version]);
  return html`<div class="claude-toast" data-testid="claude-toast" data-host=${c.host} data-phase=${c.phase} role=${c.phase === 'failed' ? 'alert' : 'status'}>
    <span class="claude-mark" aria-hidden="true">${c.phase === 'done' ? '✓' : c.phase === 'failed' ? '×' : busy ? '…' : '↓'}</span>
    <div class="claude-body">
      <span class="claude-text">${claudeText(c)}</span>
      ${busy ? html`<span class="claude-progress" aria-hidden="true"><i></i></span>` : null}
      ${c.tail ? html`<pre class="claude-tail">${c.tail}</pre>` : null}
    </div>
    ${offer ? html`<span class="claude-actions">
      <button type="button" class="btn btn-sm btn-primary" data-testid="claude-accept" onClick=${() => act(c, c.phase === 'offer_install')}>${c.phase === 'offer_install' ? 'Install' : 'Update'}</button>
      <button type="button" class="btn btn-sm" data-testid="claude-skip" onClick=${() => skip(c)}>Skip</button></span>` : null}
    ${!busy && !offer ? html`<button type="button" class="icon-btn" aria-label="dismiss" onClick=${() => close(c)}>×</button>` : null}
  </div>`;
}

export function ClaudeToastList() {
  const list = Object.values(model.value.claude).filter((c) => c && c.phase && shown(c));
  return html`${list.map((c) => html`<${ClaudeToast} key=${c.host} c=${c} />`)}`;
}
