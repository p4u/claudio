// Conversation search: `/` or the palette. Hits come from every host with the search feature.
import { html } from 'htm/preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { model, reqQuiet } from '../store.js';
import { overlay, defineAction } from '../palette.js';
import { Dialog } from './modal.js';
import { toast } from './toasts.js';
import { ago } from './glyph.js';
import { openSession } from './session-actions.js';
import { openWizard } from './wizard.js';

export function openSearch() {
  overlay.value = 'search';
}

// A live session on that conversation gets focus; otherwise the wizard opens filled in to resume it.
export function openConversation({ host, cwd, conversation }) {
  const live = Object.values(model.value.sessions).find((s) => s.host === host && s.claude_session_id === conversation && !s.dormant && s.state !== 'exited');
  if (live) { overlay.value = null; openSession(live.id); return; }
  openWizard({ host, cwd, resume: conversation, kind: 'claude' });
}

export function Search() {
  const [q, setQ] = useState('');
  const [res, setRes] = useState(null); // { hits, failed, truncated } | null
  const [busy, setBusy] = useState(false);
  const [sel, setSel] = useState(0);
  const seq = useRef(0);
  const input = useRef(null);
  const list = useRef(null);

  function run(query) {
    const mine = ++seq.current;
    const text = query.trim();
    if (!text) { setRes(null); setBusy(false); return; }
    setBusy(true);
    reqQuiet('search', { query: text })
      .then((r) => { if (mine === seq.current) { setRes({ hits: r.hits ?? [], failed: r.failed ?? [], truncated: !!r.truncated }); setSel(0); } })
      .catch((e) => { if (mine === seq.current) { setRes({ hits: [], failed: [], truncated: false, error: e.message }); } })
      .finally(() => { if (mine === seq.current) setBusy(false); });
  }
  useEffect(() => { const t = setTimeout(() => run(q), 280); return () => clearTimeout(t); }, [q]);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [sel, res]);

  const hits = res?.hits ?? [];
  const choose = (h) => { if (h) openConversation(h); };
  const key = (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); setSel((sel + 1) % Math.max(hits.length, 1)); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setSel((sel - 1 + hits.length) % Math.max(hits.length, 1)); }
    else if (e.key === 'Enter') {
      e.preventDefault();
      if (hits.length && res && q.trim()) choose(hits[sel]);
      else run(q);
    }
  };
  return html`<${Dialog} title="Search conversations" testid="search" wide onClose=${() => { overlay.value = null; }}>
    <input ref=${input} class="input search-input" data-testid="search-input" data-autofocus value=${q} placeholder="Search every host's past conversations" autocomplete="off" spellcheck=${false}
      onInput=${(e) => setQ(e.target.value)} onKeyDown=${key} />
    <div class="search-state" aria-live="polite">
      ${busy ? 'Searching…' : res?.error ? html`<span class="wz-warn">${res.error}</span>` : res && !hits.length && q.trim() ? 'No conversation matches.' : !q.trim() ? 'Type a word or a phrase. Enter opens the highlighted result.' : ''}
    </div>
    <ul class="search-hits" ref=${list} role="listbox">
      ${hits.map((h, i) => html`<li key=${`${h.host}:${h.conversation}`} class="search-hit" data-testid="search-hit" role="option" aria-selected=${i === sel}
        onMouseMove=${() => sel !== i && setSel(i)} onClick=${() => choose(h)}>
        <div class="hit-top"><span class="chip">${h.host}</span><b class="hit-title">${h.title || '(untitled)'}</b><span class="hit-age">${ago(h.modified)}</span></div>
        <div class="hit-snippet">${h.snippet}</div>
        <div class="hit-foot"><code>${h.cwd}</code>${h.matches > 1 ? html`<span>${h.matches} matches</span>` : null}</div>
      </li>`)}
    </ul>
    ${(res?.failed ?? []).map((f) => html`<p class="wz-note wz-warn" key=${f.host}>${f.host}: ${f.error}</p>`)}
    ${res?.truncated ? html`<p class="wz-note">More conversations match; narrow the search.</p>` : null}
  <//>`;
}

export function installSearch() {
  defineAction('search', openSearch);
}
