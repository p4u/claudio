// State glyphs and ages shared by the rail, the queue, the board and the panes.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { isElicitation, stateClass } from '../queue.js';

const SPIN = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
export const STATE_NAME = { working: 'working', needs_approval: 'needs approval', needs_input: 'needs input', idle: 'idle', error: 'error', exited: 'exited', starting: 'starting', unknown: 'reconnecting' };

const spin = signal(0);
export const now = signal(Date.now());
export const reduced = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
if (!reduced) setInterval(() => { spin.value = (spin.value + 1) % SPIN.length; }, 100);
setInterval(() => { now.value = Date.now(); }, 20000);

// 'input' for a question the session asks (the amber `?`), else null.
export const tone = (s) => (s.state === 'needs_approval' && isElicitation(s) ? 'input' : null);

// What each class shows: working = the spinner, waiting = a pulsing `?`, stopped = `■`, error = `✗`.
const CLASS_GLYPH = { waiting: '?', stopped: '■', error: '✗' };
export const CLASS_NAME = { working: 'working', waiting: 'waiting for you', stopped: 'stopped', error: 'error' };
export const CLASS_TIP = {
  working: 'Working', waiting: 'Waiting for you: claude asks something', stopped: 'Stopped: claude finished or is not running', error: 'Error',
};

export function glyphFor(s) {
  if (s.kind === 'shell' && s.state !== 'exited') return '$';
  if (s.state === 'unknown') return '⇄';
  const c = stateClass(s);
  if (c === 'working') return reduced ? '⠸' : SPIN[spin.value];
  return CLASS_GLYPH[c];
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
  const c = stateClass(s);
  const name = s.kind === 'shell' ? 'terminal' : s.state === 'unknown' ? 'reconnecting' : CLASS_NAME[c];
  return html`<span class="glyph" role="img" data-class=${c} data-tone=${tone(s)} aria-label=${name} title=${s.state === 'unknown' ? 'Reconnecting: its host is not connected' : CLASS_TIP[c]}>${glyphFor(s)}</span>`;
}
