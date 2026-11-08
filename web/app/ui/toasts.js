import { signal } from '@preact/signals';
import { html } from 'htm/preact';

export const toasts = signal([]);
let next = 1;

export function dismiss(id) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

// toast({text, level: 'info'|'warn'|'error', actions?: [{label, run}], sticky?}) -> id
export function toast({ text, level = 'info', actions = [], sticky = false }) {
  const id = next++;
  toasts.value = [...toasts.value.slice(-5), { id, text: String(text), level, actions, sticky }];
  if (!sticky) setTimeout(() => dismiss(id), level === 'error' ? 9000 : 5000);
  return id;
}

const MARK = { info: 'i', warn: '!', error: '×' };

export function Toasts() {
  return html`<div class="toasts" aria-live="polite">
    ${toasts.value.map((t) => html`<div key=${t.id} class=${`toast toast-${t.level}`} data-testid="toast" data-level=${t.level} role=${t.level === 'error' ? 'alert' : 'status'}>
      <span class="toast-mark" aria-hidden="true">${MARK[t.level] ?? 'i'}</span>
      <span class="toast-text">${t.text}</span>
      ${t.actions.map((a) => html`<button class="btn btn-sm" onClick=${() => { dismiss(t.id); a.run(); }}>${a.label}</button>`)}
      <button class="icon-btn" aria-label="dismiss" onClick=${() => dismiss(t.id)}>×</button>
    </div>`)}
  </div>`;
}
