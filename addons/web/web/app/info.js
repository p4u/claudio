// The hover info of a session: its facts as text rows, and where the popover goes. Pure. Imports
// only the launch label.
import { labelFor } from './launch.js';

export const INFO_DELAY_MS = 800;

const CLASS_NAME = { working: 'working', waiting: 'waiting for you', stopped: 'stopped', error: 'error' };

export function duration(sec) {
  const d = Math.max(0, Math.floor(sec));
  if (d < 60) return `${d}s`;
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ${Math.floor((d % 3600) / 60)}m`;
  return `${Math.floor(d / 86400)}d ${Math.floor((d % 86400) / 3600)}h`;
}

const tokens = (n) => {
  if (!Number.isFinite(n)) return '';
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e5 ? 0 : 1)}k`;
  return String(n);
};

// git counts: +staged ~unstaged ?untracked !conflicts ↑ahead ↓behind
function counts(g) {
  if (!g) return '';
  return [['+', g.staged], ['~', g.unstaged], ['?', g.untracked], ['!', g.conflicts], ['↑', g.ahead], ['↓', g.behind]]
    .filter(([, n]) => n).map(([c, n]) => `${c}${n}`).join(' ');
}

// -> [{k, v}] of strings; rows without a value are left out. `cls` is stateClass(session),
// `window` the context window in tokens (for the percentage); nowSec is Unix seconds.
export function infoRows(s, cls, nowSec, window = 200_000) {
  const rows = [];
  const add = (k, v) => { if (v !== null && v !== undefined && v !== '') rows.push({ k, v: String(v) }); };
  const a = s.attention;
  const ask = cls === 'waiting' && a ? [a.message, a.tool, a.detail].filter(Boolean).join(' · ') : '';
  const stateName = s.state === 'unknown' ? 'reconnecting' : (CLASS_NAME[cls] ?? s.state);
  add('Name', s.label);
  add('State', ask ? `${stateName}: ${ask}` : `${stateName} (${s.state})`);
  add('Host', s.host);
  add('Folder', s.cwd);
  const git = counts(s.git);
  add('Branch', s.branch ? `${s.branch}${git ? `  ${git}` : ''}` : '');
  if (s.kind !== 'shell') {
    add('Model', s.model);
    if (Number.isFinite(s.context_tokens)) add('Context', `${tokens(s.context_tokens)} tokens (${Math.round(Math.min(1, s.context_tokens / window) * 100)}%)`);
    if (Number.isFinite(s.output_tokens)) add('Output', `${tokens(s.output_tokens)} tokens`);
    if (Number.isFinite(s.turns)) add('Turns', s.turns);
  }
  if (s.created_at) add('Uptime', duration(nowSec - s.created_at));
  if (s.state_since) add('In this state', duration(nowSec - s.state_since));
  if (s.kind !== 'shell') add('Launch', labelFor(s.launch));
  add('Kind', s.kind === 'shell' ? 'terminal' : 'claude');
  return rows;
}

// Where the popover goes: right of the item, else left, else below, else above; never over the
// item, always inside the viewport. `vertical` (a wide row) tries below, then above. rect = {left, top, right, bottom}; size = {w, h}; vp = {w, h}.
export function placeTip(rect, size, vp, gap = 8, vertical = false) {
  const clampY = (y) => Math.max(gap, Math.min(y, vp.h - size.h - gap));
  const clampX = (x) => Math.max(gap, Math.min(x, vp.w - size.w - gap));
  if (!vertical && rect.right + gap + size.w <= vp.w - gap) return { left: rect.right + gap, top: clampY(rect.top) };
  if (!vertical && rect.left - gap - size.w >= gap) return { left: rect.left - gap - size.w, top: clampY(rect.top) };
  if (rect.bottom + gap + size.h <= vp.h - gap) return { left: clampX(rect.left), top: rect.bottom + gap };
  return { left: clampX(rect.left), top: Math.max(gap, rect.top - gap - size.h) };
}
