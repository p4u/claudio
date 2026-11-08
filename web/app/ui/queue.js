// The attention queue: what needs the person, with Enter/Esc on permission prompts only.
import { html } from 'htm/preact';
import { useState } from 'preact/hooks';
import { model, fresh, reqQuiet } from '../store.js';
import { attentionQueue, cardActions, approveArgs } from '../queue.js';
import { Glyph, ago, now, STATE_NAME } from './glyph.js';
import { confirm } from './modal.js';
import { toast } from './toasts.js';
import { narrow } from './narrow.js';
import { openSession } from './session-actions.js';

const KIND_LABEL = {
  permission_prompt: 'permission', worker_permission_prompt: 'worker permission',
  elicitation_dialog: 'question', elicitation_url_dialog: 'link request',
  idle_prompt: 'waiting for input', agent_needs_input: 'agent needs input', session_start: 'started',
};
const FALLBACK = { needs_approval: 'Waiting for approval', needs_input: 'Waiting for your input', error: 'Stopped with an error', idle: 'Finished' };

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

export function QueueCard({ s }) {
  const [busy, setBusy] = useState(false);
  const host = model.value.hosts[s.host];
  const a = s.attention;
  const acts = cardActions(s, host);
  const detail = [a?.tool, a?.detail].filter(Boolean).join('  ');
  const message = a?.message || FALLBACK[s.state] || STATE_NAME[s.state];
  return html`<article class="qcard" data-testid="queue-card" data-sid=${s.id} data-state=${s.state} data-kind=${a?.kind ?? s.state} data-fresh=${!!fresh.value[s.id]}>
    <div class="card-row"><${Glyph} s=${s} /><span class="card-label" title=${s.label}>${s.label}</span>
      <span class="chip">${s.host}</span><span class="card-age">${ago(s.state_since, now.value)}</span></div>
    <div class="qcard-kind">${a?.kind ? (KIND_LABEL[a.kind] ?? a.kind.replace(/_/g, ' ')) : STATE_NAME[s.state]}</div>
    <div class="qcard-msg" title=${message}>${message}</div>
    ${detail ? html`<div class="qcard-detail" title=${detail}>${detail}</div>` : null}
    ${acts.warning ? html`<div class="qcard-warn">${acts.warning}</div>` : null}
    <div class="qcard-foot">
      <button class="btn btn-sm" data-testid="open-btn" onClick=${() => openSession(s.id)}>Open</button>
      <span class="qcard-actions">
        ${acts.deny ? html`<button class="btn btn-sm btn-deny" data-testid="deny-btn" disabled=${busy} title="Deny: sends Esc to the terminal" aria-label="Deny, sends Escape" onClick=${() => decide(s, host, 'deny', setBusy)}>Esc</button>` : null}
        ${acts.approve ? html`<button class="btn btn-sm btn-primary" data-testid="approve-btn" disabled=${busy} title="Approve: sends Enter to the terminal" aria-label="Approve, sends Enter" onClick=${() => decide(s, host, 'approve', setBusy)}>Enter</button>` : null}
      </span>
    </div>
  </article>`;
}

export function Queue() {
  const list = queueSessions();
  return html`<div class="queue-wrap">
    <div class="section-head"><span>Needs you</span><span class="count" data-testid="queue-count">${list.length}</span></div>
    <div class="queue" data-testid="queue">
      ${list.length === 0 ? html`<p class="rail-empty">Nothing waiting. All quiet.</p>` : list.map((s) => html`<${QueueCard} key=${s.id} s=${s} />`)}
    </div>
  </div>`;
}
