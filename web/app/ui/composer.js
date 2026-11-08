// Prompts. The terminal is the one input by default; this file has the Compose pop-up editor for
// long or multi-line prompts and the optional inline composer ("Show a composer under each pane").
// Both only ever talk to claude with a bracketed paste (`session.paste`), plus Enter when sending.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useRef, useState } from 'preact/hooks';
import { model, focused, req } from '../store.js';
import { focusTerminal } from '../term.js';
import { createDrafts, tooLarge } from '../compose.js';
import { overlay, defineAction, register } from '../palette.js';
import { Dialog } from './modal.js';
import { toast } from './toasts.js';

export const drafts = createDrafts(); // sid -> unsent text, kept while the editor is closed
export const composeSid = signal(null);

export const isLive = (s) => !s.dormant && s.state !== 'exited' && s.state !== 'unknown' && s.state !== 'starting';

const OVER = 'That is over 64 KiB. Paste it in pieces.';

// Paste `text` into the session; false when it was not sent (the caller keeps the draft).
async function paste(sid, text, submit) {
  if (tooLarge(text)) { toast({ text: OVER, level: 'warn' }); return false; }
  try {
    await req('session.paste', { sid, text, submit });
    return true;
  } catch { return false; /* req already told the person */ }
}

// The session Compose targets: the focused claude session, or null (and a notice).
function target() {
  const s = model.value.sessions[focused.value];
  if (!s || s.kind !== 'claude') { toast({ text: 'Focus a claude session first.', level: 'warn' }); return null; }
  return s;
}

export function openCompose(sid) {
  const s = sid ? model.value.sessions[sid] : target();
  if (!s || s.kind !== 'claude') return;
  composeSid.value = s.id;
  overlay.value = 'compose';
}

export function closeCompose() {
  if (overlay.value === 'compose') overlay.value = null;
}

// Snippets land in the focused claude session's draft; the editor opens to show them.
export function insertIntoComposer(text) {
  const s = target();
  if (!s) return false;
  drafts.append(s.id, text);
  openCompose(s.id);
  return true;
}

// What "save as snippet" saves: the focused claude session's draft.
export const focusedDraft = () => {
  const s = model.value.sessions[focused.value];
  return s?.kind === 'claude' ? drafts.get(s.id) : '';
};

export function ComposeDialog() {
  const sid = composeSid.value;
  const s = model.value.sessions[sid];
  const ref = useRef(null);
  const [text, setText] = useState(drafts.get(sid));
  const [busy, setBusy] = useState(false);
  if (!s) { queueMicrotask(closeCompose); return null; }
  const live = isLive(s);
  const over = tooLarge(text);
  const can = live && !busy && !over && text.trim() !== '';

  const change = (e) => { setText(e.target.value); drafts.set(sid, e.target.value); };
  async function go(submit) {
    if (!can) return;
    setBusy(true);
    const ok = await paste(sid, text, submit);
    setBusy(false);
    if (!ok) return;
    drafts.clear(sid);
    closeCompose();
    setTimeout(() => focusTerminal(sid), 0);
  }
  const key = (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); go(true); }
  };
  return html`<${Dialog} title=${`Compose prompt · ${s.label}`} testid="compose" wide onClose=${closeCompose}>
    <textarea ref=${ref} class="compose-text" data-testid="composer" data-autofocus value=${text} disabled=${!live}
      placeholder=${live ? 'Write the prompt. Enter adds a line.' : 'Not running. Resume it to write a prompt.'}
      aria-label=${`Prompt for ${s.label}`} spellcheck=${false} onInput=${change} onKeyDown=${key}></textarea>
    ${over ? html`<p class="wz-warn" role="alert" data-testid="compose-over">${OVER}</p>` : null}
    <div class="dialog-actions">
      <span class="compose-hint">Ctrl+Enter sends · Esc keeps the draft</span>
      <button type="button" class="btn" data-testid="compose-insert" disabled=${!can} onClick=${() => go(false)} title="Put the text in claude's input box without sending it">Insert</button>
      <button type="button" class="btn btn-primary" data-testid="compose-send" disabled=${!can} onClick=${() => go(true)} title="Paste it and press Enter (Ctrl+Enter)">Send</button>
    </div>
  <//>`;
}

// The inline composer under a pane's terminal (setting `inlineComposer`): a textarea that submits as one paste.
export function Composer({ s }) {
  const ref = useRef(null);
  const [text, setText] = useState(drafts.get(s.id));
  const [busy, setBusy] = useState(false);
  const live = isLive(s);

  const grow = (el) => { el.style.height = 'auto'; el.style.height = `${Math.min(el.scrollHeight, 168)}px`; };
  const change = (e) => {
    setText(e.target.value);
    drafts.set(s.id, e.target.value);
    grow(e.target);
  };

  async function send() {
    if (!live || busy || text.trim() === '') return;
    setBusy(true);
    if (await paste(s.id, text, true)) {
      drafts.clear(s.id);
      setText('');
      if (ref.current) ref.current.style.height = 'auto';
    }
    setBusy(false);
  }

  const key = (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); focusTerminal(s.id); }
  };

  return html`<div class="composer-wrap">
    <textarea ref=${ref} class="composer" data-testid="composer-inline" rows="1" value=${text} disabled=${!live}
      placeholder=${live ? 'Prompt…' : 'Not running. Resume it to write a prompt.'}
      aria-label="Prompt composer" onInput=${change} onKeyDown=${key}></textarea>
    <button class="btn btn-sm btn-primary" data-testid="composer-send" disabled=${!live || busy || text.trim() === ''} onClick=${send} title="Ctrl+Enter sends. Shift+Enter adds a line. Esc returns to the terminal.">Send</button>
  </div>`;
}

export function installCompose() {
  defineAction('compose', () => { if (overlay.value === 'compose') closeCompose(); else openCompose(); });
  register({ id: 'compose', group: 'Prompt', title: 'Compose prompt', keys: 'Alt+O', run: () => openCompose() });
}
