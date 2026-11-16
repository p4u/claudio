// The wizard's "Find a conversation…" field: the `search` op over every connected host, its hits
// under the field; picking one fills the wizard to resume it (or focuses its live session).
import { html } from 'htm/preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { reqQuiet } from '../store.js';
import { searchable, searchResult, moveSel } from '../hits.js';
import { abbreviateHome } from '../dirnav.js';
import { ago } from './glyph.js';

// `onPick(hit)`: a hit was chosen. `homes`: known host homes, for `~/`.
export function FindField({ onPick, homes }) {
  const [q, setQ] = useState('');
  const [res, setRes] = useState(null);
  const [busy, setBusy] = useState(false);
  const [sel, setSel] = useState(0);
  const seq = useRef(0);

  function run(query) {
    const mine = ++seq.current;
    if (!searchable(query)) { setRes(null); setBusy(false); return; }
    setBusy(true);
    reqQuiet('search', { query: query.trim() })
      .then((r) => { if (mine === seq.current) { setRes(searchResult(r)); setSel(0); } })
      .catch((e) => { if (mine === seq.current) setRes({ ...searchResult(null), error: e.message }); })
      .finally(() => { if (mine === seq.current) setBusy(false); });
  }
  useEffect(() => { const t = setTimeout(() => run(q), 280); return () => clearTimeout(t); }, [q]);

  const hits = res?.hits ?? [];
  const choose = (h) => { if (!h) return; seq.current += 1; setQ(''); setRes(null); onPick(h); };
  const key = (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); setSel(moveSel(sel, e.key === 'ArrowDown' ? 1 : -1, hits.length)); }
    else if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); choose(hits[sel]); }
    else if (e.key === 'Escape' && q) { e.preventDefault(); e.stopPropagation(); seq.current += 1; setQ(''); setRes(null); }
  };
  const state = busy ? 'Searching…' : res?.error ? res.error : res && !hits.length ? 'No conversation matches.' : null;
  return html`<div class="wz-field">
    <label for="wz-find">Find a conversation</label>
    <input id="wz-find" class="input" data-testid="wizard-find" value=${q} placeholder="Find a conversation…" autocomplete="off" spellcheck=${false}
      onInput=${(e) => setQ(e.target.value)} onKeyDown=${key} />
    ${state ? html`<p class=${`wz-note${res?.error ? ' wz-warn' : ''}`} aria-live="polite">${state}</p>` : null}
    ${hits.length ? html`<ul class="search-hits wz-find-hits" role="listbox">
      ${hits.map((h, i) => html`<li key=${`${h.host}:${h.conversation}`} class="search-hit" role="option" data-testid="wizard-find-row" data-conversation=${h.conversation} aria-selected=${i === sel}
        onMouseMove=${() => sel !== i && setSel(i)} onClick=${() => choose(h)}>
        <div class="hit-top"><b class="hit-title">${h.title || '(untitled)'}</b></div>
        <div class="hit-foot"><code>${h.host} · ${abbreviateHome(h.cwd, homes[h.host])} · ${ago(h.modified)}</code></div>
        <div class="hit-snippet">${h.snippet}</div>
      </li>`)}
    </ul>` : null}
    ${(res?.failed ?? []).map((f) => html`<p class="wz-note wz-warn" key=${f.host}>${f.host}: ${f.error}</p>`)}
  </div>`;
}
