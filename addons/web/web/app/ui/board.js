// The board: one row per session, shown when no pane is open.
import { html } from 'htm/preact';
import { model, selected, focused, fresh } from '../store.js';
import { orderedSessions } from '../model.js';
import { stateClass } from '../queue.js';
import { Glyph, ago, now, tone, CLASS_NAME } from './glyph.js';
import { ApproveButtons } from './approve.js';
import { infoProps } from './info.js';
import { Gauge, Branch } from './strip.js';
import { openSession, openAndEnter, dropSession } from './session-actions.js';
import { SID_TYPE, dragSource } from './rail.js';
import { mergeHandlers } from '../props.js';
import { narrow } from './narrow.js';

function Row({ s }) {
  const current = selected.value === s.id || focused.value === s.id;
  const attn = s.attention ? [s.attention.message, s.attention.detail].filter(Boolean).join(' · ') : '';
  return html`<div class="board-row" role="row" tabindex="0" data-testid="board-row" data-sid=${s.id} data-state=${s.state} data-class=${stateClass(s)} data-kind=${s.kind}
    data-tone=${tone(s)} data-fresh=${!!fresh.value[s.id]} aria-current=${current} aria-selected=${selected.value === s.id} onClick=${() => openSession(s.id)}
    onKeyDown=${(e) => { if (e.target === e.currentTarget && e.key === 'Enter') { e.preventDefault(); openAndEnter(s.id); } }} ...${mergeHandlers(dragSource(s), infoProps(s, true))}>
    <span class="bcol bcol-state"><span class="badge bstate" data-class=${stateClass(s)} data-kind=${s.kind}><${Glyph} s=${s} />${s.kind === 'shell' ? 'terminal' : s.state === 'unknown' ? 'reconnecting' : stateClass(s) === 'waiting' ? 'waiting' : CLASS_NAME[stateClass(s)]}</span></span>
    <span class="bcol bcol-label" title=${s.label}>${s.label}</span>
    <span class="bcol bcol-host"><span class="chip">${s.host}</span></span>
    <span class="bcol bcol-repo" title=${s.cwd}>${s.branch ? html`<${Branch} s=${s} />` : html`<span class="muted">${s.cwd}</span>`}</span>
    <span class="bcol bcol-attn"><span class="attn-text" title=${attn}>${attn}</span><${ApproveButtons} s=${s} /></span>
    <span class="bcol bcol-model">${s.model ?? ''}</span>
    <span class="bcol bcol-ctx">${s.kind === 'claude' ? html`<${Gauge} s=${s} />` : null}</span>
    <span class="bcol bcol-age">${ago(s.state_since, now.value)}</span>
  </div>`;
}

export function Board() {
  const list = orderedSessions(model.value);
  return html`<main class="board" data-testid="board" role="table" aria-label="Sessions"
    onDragOver=${(e) => { if (e.dataTransfer.types.includes(SID_TYPE)) e.preventDefault(); }}
    onDrop=${(e) => { const sid = e.dataTransfer.getData(SID_TYPE); if (sid && !narrow.value) { e.preventDefault(); dropSession(model.value.layout.focus, sid); } }}>
    <div class="board-head" role="row">
      <span class="bcol bcol-state">state</span><span class="bcol bcol-label">session</span><span class="bcol bcol-host">host</span>
      <span class="bcol bcol-repo">repo</span><span class="bcol bcol-attn">needs</span><span class="bcol bcol-model">model</span><span class="bcol bcol-ctx">context</span><span class="bcol bcol-age">age</span>
    </div>
    <div class="board-rows">
      ${list.length === 0 ? html`<div class="pane-empty"><div class="coin">PRESS <kbd>N</kbd> TO START</div>
        <div class="hint"><span><kbd>N</kbd> new session</span><span><kbd>T</kbd> terminal</span><span><kbd>K</kbd> palette</span><span><kbd>?</kbd> keys</span></div></div>` : null}
      ${list.map((s) => html`<${Row} key=${s.id} s=${s} />`)}
    </div>
  </main>`;
}
