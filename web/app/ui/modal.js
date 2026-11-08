import { signal } from '@preact/signals';
import { html } from 'htm/preact';
import { useLayoutEffect, useRef } from 'preact/hooks';

export const modal = signal(null);

function open(spec) {
  return new Promise((resolve) => {
    modal.value?.finish(modal.value.cancelValue);
    const entry = { ...spec };
    entry.finish = (v) => {
      if (modal.value === entry) modal.value = null;
      resolve(v);
    };
    modal.value = entry;
  });
}

// confirm({title, body, yes, no}) -> Promise<boolean>
export function confirm({ title, body = '', yes = 'Yes', no = 'No' }) {
  return open({ kind: 'confirm', title, body, yes, no, cancelValue: false });
}

// prompt({title, value, yes, no}) -> Promise<string|null>
export function prompt({ title, value = '', yes = 'OK', no = 'Cancel', placeholder = '', testid = 'rename-input' }) {
  return open({ kind: 'prompt', title, value, yes, no, placeholder, testid, cancelValue: null });
}

export function closeModal() {
  modal.value?.finish(modal.value.cancelValue);
}

export function Modal() {
  const m = modal.value;
  const ref = useRef(null);
  useLayoutEffect(() => { ref.current?.focus(); }, [m]);
  if (!m) return null;
  const isPrompt = m.kind === 'prompt';
  const submit = () => m.finish(isPrompt ? ref.current.value : true);
  return html`<div class="scrim" onMouseDown=${(e) => { if (e.target === e.currentTarget) closeModal(); }}>
    <div class="dialog popover" role="dialog" aria-modal="true" aria-label=${m.title}
      onKeyDown=${(e) => { if (e.key === 'Escape') { e.stopPropagation(); closeModal(); } if (e.key === 'Enter' && isPrompt) submit(); }}>
      <h2 class="pixel dialog-title">${m.title}</h2>
      ${m.body ? html`<p class="dialog-body">${m.body}</p>` : null}
      ${isPrompt ? html`<input class="input" ref=${ref} data-testid=${m.testid} value=${m.value} placeholder=${m.placeholder} />` : null}
      <div class="dialog-actions">
        <button class="btn" data-testid="confirm-no" onClick=${closeModal}>${m.no}</button>
        <button class="btn btn-primary" data-testid="confirm-yes" ref=${isPrompt ? null : ref} onClick=${submit}>${m.yes}</button>
      </div>
    </div>
  </div>`;
}

// A titled popover used by help and settings: Esc closes, Tab stays inside, focus returns.
export function Dialog({ title, onClose, testid, wide = false, children }) {
  const root = useRef(null);
  useLayoutEffect(() => {
    const prev = document.activeElement;
    (root.current.querySelector('[data-autofocus]') ?? root.current).focus();
    return () => { if (prev && prev.isConnected) prev.focus(); };
  }, []);
  const keydown = (e) => {
    if (e.key === 'Escape') { e.stopPropagation(); onClose(); return; }
    if (e.key !== 'Tab') return;
    const f = [...root.current.querySelectorAll('button, input, select, textarea, [tabindex="0"]')].filter((x) => !x.disabled);
    if (!f.length) return;
    const first = f[0];
    const last = f[f.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  };
  return html`<div class="scrim" onMouseDown=${(e) => { if (e.target === e.currentTarget) onClose(); }}>
    <div class=${`dialog popover${wide ? ' wide' : ''}`} ref=${root} tabindex="-1" role="dialog" aria-modal="true" aria-label=${title} data-testid=${testid} onKeyDown=${keydown}>
      <h2 class="pixel dialog-title"><span>${title}</span><button class="icon-btn" aria-label="close" onClick=${onClose}>×</button></h2>
      ${children}
    </div>
  </div>`;
}
