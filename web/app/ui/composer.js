// Prompt composer under a claude terminal: a real textarea that submits as one paste.
import { html } from 'htm/preact';
import { useRef, useState } from 'preact/hooks';
import { req } from '../store.js';
import { focusTerminal } from '../term.js';
import { toast } from './toasts.js';

const MAX = 64 * 1024;
const drafts = new Map(); // sid -> unsent text, kept while the pane is away
const enc = new TextEncoder();

export const isLive = (s) => !s.dormant && s.state !== 'exited' && s.state !== 'unknown' && s.state !== 'starting';

export function Composer({ s }) {
  const ref = useRef(null);
  const [text, setText] = useState(drafts.get(s.id) ?? '');
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
    if (enc.encode(text).length > MAX) { toast({ text: 'That is over 64 KiB. Paste it in pieces.', level: 'warn' }); return; }
    setBusy(true);
    try {
      await req('session.paste', { sid: s.id, text, submit: true });
      drafts.delete(s.id);
      setText('');
      if (ref.current) ref.current.style.height = 'auto';
    } catch { /* req already told the person; the draft stays */ }
    setBusy(false);
  }

  const key = (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); focusTerminal(s.id); }
  };

  return html`<div class="composer-wrap">
    <textarea ref=${ref} class="composer" data-testid="composer" rows="1" value=${text} disabled=${!live}
      placeholder=${live ? 'Prompt. Ctrl+Enter sends, Esc returns to the terminal.' : 'Not running. Resume it to write a prompt.'}
      aria-label="Prompt composer" onInput=${change} onKeyDown=${key}></textarea>
    <button class="btn btn-sm btn-primary" data-testid="composer-send" disabled=${!live || busy || text.trim() === ''} onClick=${send} title="Ctrl+Enter">Send</button>
  </div>`;
}
