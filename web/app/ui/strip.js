// The per-session facts: the context gauge, git counts and the status strip.
import { html } from 'htm/preact';
import { segments, level, ctxFraction } from '../gauge.js';

export function fmtTokens(n) {
  if (!Number.isFinite(n)) return '';
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}

// +staged ~unstaged ?untracked !conflicts ↑ahead ↓behind, nonzero parts only.
export function gitText(g) {
  if (!g) return '';
  const parts = [];
  if (g.staged) parts.push(`+${g.staged}`);
  if (g.unstaged) parts.push(`~${g.unstaged}`);
  if (g.untracked) parts.push(`?${g.untracked}`);
  if (g.conflicts) parts.push(`!${g.conflicts}`);
  if (g.ahead) parts.push(`↑${g.ahead}`);
  if (g.behind) parts.push(`↓${g.behind}`);
  return parts.join(' ');
}

// A segmented meter: one cell per segment of segments(), lit cells in the level's colour.
export function SegBar({ f, n = 8 }) {
  return html`<span class="gauge-bar" aria-hidden="true">${[...segments(f, n)].map((c, i) => html`<i key=${i} data-on=${c === '█'}></i>`)}</span>`;
}

export function Gauge({ s, bare = false }) {
  const f = ctxFraction(s);
  if (f === null) return null;
  const pct = Math.round(f * 100);
  return html`<span class="gauge" data-testid="gauge" data-level=${level(f)} title=${`context ${fmtTokens(s.context_tokens)} (${pct}%)`}>
    <${SegBar} f=${f} />${bare ? null : html`<span class="gauge-pct">${pct}%</span>`}</span>`;
}

export function Branch({ s }) {
  if (!s.branch) return null;
  const g = gitText(s.git);
  return html`<span class="branch" title=${g ? `${s.branch}  ${g}` : s.branch}>⎇ ${s.branch}${g ? html` <span class="git-counts">${g}</span>` : null}</span>`;
}

export function Strip({ s }) {
  if (s.kind === 'shell') {
    return html`<footer class="strip" data-testid="strip"><span class="strip-item" title=${s.cwd}>${s.cwd}</span><span class="strip-item muted">${s.host}</span></footer>`;
  }
  return html`<footer class="strip" data-testid="strip">
    ${s.model ? html`<span class="strip-item">${s.model}</span>` : null}
    <${Gauge} s=${s} />
    ${s.branch ? html`<${Branch} s=${s} />` : null}
    ${s.output_tokens != null ? html`<span class="strip-item" title="output tokens">out ${fmtTokens(s.output_tokens)}</span>` : null}
    ${s.turns != null ? html`<span class="strip-item">${s.turns} turn${s.turns === 1 ? '' : 's'}</span>` : null}
    ${s.prompt_pending ? html`<span class="strip-item strip-pending" data-testid="prompt-pending">opening prompt pending</span>` : null}
  </footer>`;
}
