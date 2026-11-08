// State glyphs and ages shared by the rail, the queue, the board and the panes.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';

const GLYPH = { working: '⠸', needs_approval: '◆', needs_input: '?', idle: '✓', error: '✗', exited: '○', starting: '·', unknown: '·' };
const SPIN = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
export const STATE_NAME = { working: 'working', needs_approval: 'needs approval', needs_input: 'needs input', idle: 'idle', error: 'error', exited: 'exited', starting: 'starting', unknown: 'reconnecting' };

const spin = signal(0);
export const now = signal(Date.now());
export const reduced = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
if (!reduced) setInterval(() => { spin.value = (spin.value + 1) % SPIN.length; }, 100);
setInterval(() => { now.value = Date.now(); }, 20000);

export function glyphFor(s) {
  if (s.kind === 'shell' && s.state !== 'exited') return '$';
  if (s.state === 'unknown') return '⇄';
  if (s.state === 'working' && !reduced) return SPIN[spin.value];
  return GLYPH[s.state] ?? '·';
}

export function ago(sec, nowMs = Date.now()) {
  if (!sec) return '';
  const d = Math.max(0, Math.floor(nowMs / 1000 - sec));
  if (d < 60) return `${d}s`;
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h`;
  return `${Math.floor(d / 86400)}d`;
}

export function Glyph({ s }) {
  return html`<span class="glyph" role="img" aria-label=${s.kind === 'shell' ? 'terminal' : STATE_NAME[s.state]}>${glyphFor(s)}</span>`;
}
