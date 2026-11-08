// The rail: the queue on top, then sessions grouped by host and folder, draggable.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, selected, focused, fresh, req } from '../store.js';
import { orderedSessions, groupSessions, applyEvent } from '../model.js';
import { moveBefore } from '../panes.js';
import { HostChip } from './topbar.js';
import { Glyph, ago, now } from './glyph.js';
import { Gauge, Branch } from './strip.js';
import { Queue } from './queue.js';
import { openSession } from './session-actions.js';

export const SID_TYPE = 'application/x-claudio-sid';
const dropMark = signal(null); // {sid, after}

// Collapsed groups survive reloads.
const COLLAPSED_KEY = 'claudio.collapsed';
function loadCollapsed() {
  try { return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY)) ?? []); } catch { return new Set(); }
}
const collapsed = signal(loadCollapsed());
function toggleGroup(key, open) {
  const next = new Set(collapsed.value);
  if (open) next.delete(key); else next.add(key);
  collapsed.value = next;
  try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next])); } catch { /* private mode */ }
}

export function reorder(sid, target, after) {
  const ids = orderedSessions(model.value).map((s) => s.id);
  let before = target;
  if (after) before = ids[ids.indexOf(target) + 1] ?? null;
  const next = moveBefore(ids, sid, before);
  if (next.join() === ids.join()) return;
  model.value = applyEvent(model.value, { ev: 'order', sids: next });
  req('session.order', { sids: next }).catch(() => {});
}

function SessionCard({ s }) {
  const mark = dropMark.value?.sid === s.id ? (dropMark.value.after ? 'after' : 'before') : null;
  const current = selected.value === s.id || focused.value === s.id;
  const open = () => openSession(s.id);
  return html`<div class="card" role="button" tabindex="0" draggable="true" data-testid="session-card" data-sid=${s.id} data-state=${s.state} data-kind=${s.kind}
    data-fresh=${!!fresh.value[s.id]} data-drop=${mark} aria-current=${current}
    onClick=${open}
    onKeyDown=${(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } }}
    onDragStart=${(e) => { e.dataTransfer.setData(SID_TYPE, s.id); e.dataTransfer.setData('text/plain', s.label); e.dataTransfer.effectAllowed = 'copyMove'; }}
    onDragOver=${(e) => {
      if (!e.dataTransfer.types.includes(SID_TYPE)) return;
      e.preventDefault();
      const r = e.currentTarget.getBoundingClientRect();
      const after = e.clientY > r.top + r.height / 2;
      if (dropMark.value?.sid !== s.id || dropMark.value.after !== after) dropMark.value = { sid: s.id, after };
    }}
    onDragLeave=${() => { if (dropMark.value?.sid === s.id) dropMark.value = null; }}
    onDrop=${(e) => {
      const sid = e.dataTransfer.getData(SID_TYPE);
      const after = dropMark.value?.after ?? false;
      dropMark.value = null;
      if (!sid) return;
      e.preventDefault();
      reorder(sid, s.id, after);
    }}
    onDragEnd=${() => { dropMark.value = null; }}>
    <span class="card-row"><${Glyph} s=${s} /><span class="card-label" title=${s.label}>${s.label}</span>
      <span class="chip" title=${`host ${s.host}`}>${s.host}</span><span class="card-age">${ago(s.state_since, now.value)}</span></span>
    <span class="card-meta">
      ${s.kind === 'shell' ? html`<span class="chip">terminal</span>` : null}
      ${s.branch ? html`<${Branch} s=${s} />` : null}
      ${s.kind === 'claude' ? html`<${Gauge} s=${s} />` : null}
      ${s.dormant ? html`<span class="chip">dormant</span>` : null}
    </span>
  </div>`;
}

export function Rail() {
  const m = model.value;
  const all = orderedSessions(m);
  const groups = groupSessions(all);
  return html`<nav class="rail" data-testid="rail" aria-label="Sessions">
    <section class="rail-section"><${Queue} /></section>
    <section class="rail-section" style="padding-bottom:0"><div class="section-head"><span>Sessions</span><span class="count">${all.length}</span></div></section>
    <div class="rail-groups">
      ${all.length === 0 ? html`<p class="rail-empty">No sessions yet.</p>` : null}
      ${groups.map((h) => html`<details class="host-group" open=${!collapsed.value.has(h.host)} key=${h.host}
        onToggle=${(e) => { if (e.target === e.currentTarget) toggleGroup(h.host, e.currentTarget.open); }}>
        <summary><div class="host-title">${m.hosts[h.host] ? html`<${HostChip} host=${m.hosts[h.host]} />` : h.host}</div></summary>
        ${h.groups.map((g) => html`<details class="cwd-group" open=${!collapsed.value.has(`${h.host}:${g.cwd}`)} key=${g.cwd}
          onToggle=${(e) => { if (e.target === e.currentTarget) toggleGroup(`${h.host}:${g.cwd}`, e.currentTarget.open); }}>
          <summary><div class="cwd-title" title=${g.cwd}><span></span><span>${g.label}</span><span class="count">${g.sessions.length}</span></div></summary>
          <div class="cards">${g.sessions.map((s) => html`<${SessionCard} key=${s.id} s=${s} />`)}</div>
        </details>`)}
      </details>`)}
    </div>
  </nav>`;
}
