// The rail: sessions grouped by host and folder, draggable. A static list: every card has the same fixed size.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, selected, focused, fresh, req } from '../store.js';
import { orderedSessions, groupSessions, railGroups, applyEvent } from '../model.js';
import { moveBefore } from '../panes.js';
import { HostChip, attentionCount } from './topbar.js';
import { stateClass } from '../queue.js';
import { Glyph, ago, now, tone } from './glyph.js';
import { infoProps } from './info.js';
import { openSession, openAndEnter } from './session-actions.js';

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

const attnLine = (s) => (s.attention ? [s.attention.message, s.attention.detail].filter(Boolean).join(' · ') : '');

function dragProps(s, mark) {
  return {
    draggable: 'true',
    onDragStart: (e) => { e.dataTransfer.setData(SID_TYPE, s.id); e.dataTransfer.setData('text/plain', s.label); e.dataTransfer.effectAllowed = 'copyMove'; },
    onDragOver: (e) => {
      if (!e.dataTransfer.types.includes(SID_TYPE)) return;
      e.preventDefault();
      const r = e.currentTarget.getBoundingClientRect();
      const after = e.clientY > r.top + r.height / 2;
      if (dropMark.value?.sid !== s.id || dropMark.value.after !== after) dropMark.value = { sid: s.id, after };
    },
    onDragLeave: () => { if (dropMark.value?.sid === s.id) dropMark.value = null; },
    onDrop: (e) => {
      const sid = e.dataTransfer.getData(SID_TYPE);
      const after = dropMark.value?.after ?? false;
      dropMark.value = null;
      if (!sid) return;
      e.preventDefault();
      reorder(sid, s.id, after);
    },
    onDragEnd: () => { dropMark.value = null; },
  };
}

// Enter opens the session and puts the keyboard in its terminal; Space only opens it.
const cardKeys = (s) => (e) => {
  if (e.key === 'Enter') { e.preventDefault(); openAndEnter(s.id); }
  else if (e.key === ' ') { e.preventDefault(); openSession(s.id); }
};

// One fixed-height line: state icon, name, host (only with more than one host), age. Everything
// else is in the hover info.
function SessionCard({ s, multiHost }) {
  const mark = dropMark.value?.sid === s.id ? (dropMark.value.after ? 'after' : 'before') : null;
  const current = selected.value === s.id || focused.value === s.id;
  return html`<div class="card" role="button" tabindex="0" data-testid="session-card" data-sid=${s.id} data-state=${s.state} data-class=${stateClass(s)} data-kind=${s.kind} data-tone=${tone(s)}
    data-fresh=${!!fresh.value[s.id]} data-drop=${mark} data-selected=${selected.value === s.id} aria-current=${current}
    onClick=${() => openSession(s.id)} onKeyDown=${cardKeys(s)} ...${dragProps(s)} ...${infoProps(s)}>
    <${Glyph} s=${s} /><span class="card-label">${s.label}</span>
    ${s.dormant ? html`<span class="chip chip-quiet">dormant</span>` : null}
    ${multiHost ? html`<span class="chip" title=${`host ${s.host}`}>${s.host}</span>` : null}<span class="card-age">${ago(s.state_since, now.value)}</span>
  </div>`;
}

// The 56 px strip: a state glyph per session and the number waiting. Hover shows the name, a click opens.
function RailStrip({ all }) {
  const waiting = attentionCount(model.value);
  return html`<nav class="rail rail-strip" data-testid="rail" data-collapsed="true" aria-label="Sessions">
    <div class="strip-head" title=${`${waiting} waiting for you`}><span class="strip-count" data-testid="queue-count" data-waiting=${waiting > 0} aria-live="polite">${waiting}</span></div>
    ${all.map((s) => {
      const attn = attnLine(s);
      const mark = dropMark.value?.sid === s.id ? (dropMark.value.after ? 'after' : 'before') : null;
      return html`<button type="button" key=${s.id} class="strip-card" data-testid="session-card" data-compact="true" data-sid=${s.id} data-state=${s.state} data-class=${stateClass(s)} data-kind=${s.kind} data-tone=${tone(s)}
        data-fresh=${!!fresh.value[s.id]} data-drop=${mark} data-selected=${selected.value === s.id} aria-current=${selected.value === s.id || focused.value === s.id}
        aria-label=${`${s.label} on ${s.host}${attn ? `: ${attn}` : ''}`}
        onClick=${() => openSession(s.id)} onKeyDown=${(e) => { if (e.key === 'Enter') { e.preventDefault(); openAndEnter(s.id); } }} ...${dragProps(s)} ...${infoProps(s)}>
        <${Glyph} s=${s} />
      </button>`;
    })}
  </nav>`;
}

export function Rail({ strip = false }) {
  const m = model.value;
  const all = orderedSessions(m);
  if (strip) return html`<${RailStrip} all=${all} />`;
  const groups = groupSessions(all);
  const multiHost = Object.keys(m.hosts).filter((n) => m.hosts[n].connected).length > 1;
  return html`<nav class="rail" data-testid="rail" aria-label="Sessions">
    <section class="rail-section" style="padding-bottom:0"><div class="section-head"><span>Sessions</span><span class="count">${all.length}</span></div></section>
    <div class="rail-groups">
      ${all.length === 0 ? html`<p class="rail-empty">No sessions yet.</p>` : null}
      ${groups.map((h) => {
        const { singles, groups: repos } = railGroups(h.groups);
        return html`<details class="host-group" open=${!collapsed.value.has(h.host)} key=${h.host}
        onToggle=${(e) => { if (e.target === e.currentTarget) toggleGroup(h.host, e.currentTarget.open); }}>
        <summary><div class="host-title">${m.hosts[h.host] ? html`<${HostChip} host=${m.hosts[h.host]} />` : h.host}<span class="count">${singles.length + repos.reduce((n, g) => n + g.sessions.length, 0)}</span></div></summary>
        ${singles.length ? html`<div class="cards">${singles.map((s) => html`<${SessionCard} key=${s.id} s=${s} multiHost=${multiHost} />`)}</div>` : null}
        ${repos.map((g) => html`<details class="cwd-group" open=${!collapsed.value.has(`${h.host}:${g.cwd}`)} key=${g.cwd}
          onToggle=${(e) => { if (e.target === e.currentTarget) toggleGroup(`${h.host}:${g.cwd}`, e.currentTarget.open); }}>
          <summary><div class="cwd-title" title=${g.cwd}><span></span><span>${g.label}</span><span class="count">${g.sessions.length}</span></div></summary>
          <div class="cards">${g.sessions.map((s) => html`<${SessionCard} key=${s.id} s=${s} multiHost=${multiHost} />`)}</div>
        </details>`)}
      </details>`;
      })}
    </div>
  </nav>`;
}
