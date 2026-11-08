// Approve / Deny for a permission prompt (Enter / Esc into the terminal), and the attention order
// that Alt+a walks. The rail no longer lists a queue; these buttons live in the pane header and the board row.
import { html } from 'htm/preact';
import { useState } from 'preact/hooks';
import { model, reqQuiet } from '../store.js';
import { attentionQueue, cardActions, approveArgs } from '../queue.js';
import { now, STATE_NAME } from './glyph.js';
import { confirm } from './modal.js';
import { toast } from './toasts.js';
import { narrow } from './narrow.js';

export function queueSessions() {
  return attentionQueue(Object.values(model.value.sessions), now.value);
}

async function decide(s, host, decision, setBusy) {
  const a = s.attention;
  if (narrow.value) {
    const ok = await confirm({
      title: decision === 'approve' ? 'Approve this?' : 'Deny this?',
      body: `${s.label}: ${a?.message ?? STATE_NAME[s.state]}${a?.detail ? ` — ${a.detail}` : ''}`,
      yes: decision === 'approve' ? 'Approve' : 'Deny', no: 'Cancel',
    });
    if (!ok) return;
  }
  setBusy(true);
  try {
    await reqQuiet('session.approve', approveArgs(s, host, decision));
  } catch (e) {
    toast({ text: e.message === 'stale' ? `${s.label}: the prompt changed before your answer arrived. Look at it again.` : `${s.label}: ${e.message}`, level: e.message === 'stale' ? 'warn' : 'error' });
  } finally {
    setBusy(false);
  }
}

// Nothing unless the session waits on a permission prompt (cardActions has the guard).
export function ApproveButtons({ s }) {
  const [busy, setBusy] = useState(false);
  const host = model.value.hosts[s.host];
  const acts = cardActions(s, host);
  if (!acts.approve && !acts.deny) return null;
  // the row and the header handle clicks and keys themselves
  const stop = (e) => e.stopPropagation();
  return html`<span class="approve-btns" onClick=${stop} onKeyDown=${stop} onPointerDown=${stop}>
    ${acts.deny ? html`<button type="button" class="btn btn-sm btn-deny" data-testid="deny-btn" disabled=${busy} title="Deny: sends Esc to the terminal" aria-label="Deny, sends Escape" onClick=${() => decide(s, host, 'deny', setBusy)}>Esc</button>` : null}
    ${acts.approve ? html`<button type="button" class="btn btn-sm btn-primary" data-testid="approve-btn" disabled=${busy} title=${acts.warning ?? 'Approve: sends Enter to the terminal'} aria-label="Approve, sends Enter" onClick=${() => decide(s, host, 'approve', setBusy)}>Enter</button>` : null}
  </span>`;
}
