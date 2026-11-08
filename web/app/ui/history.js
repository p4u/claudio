// The History tab: git log pages, a commit's details and its patches (same diff view as Changes).
import { html } from 'htm/preact';
import { useEffect, useState } from 'preact/hooks';
import { model, reqQuiet } from '../store.js';
import { defineAction } from '../palette.js';
import { registerDrawerTab, toggleDrawer } from './drawer.js';
import { DiffView, ModeSwitch, diffMode } from './changes.js';
import { ago } from './glyph.js';

const PAGE = 100;

// 'HEAD -> refs/heads/main' -> {label: 'main', kind: 'head'}, 'tag: refs/tags/v1' -> tag, remotes -> remote.
export function refChip(ref) {
  let r = String(ref);
  let kind = 'branch';
  if (r.startsWith('HEAD -> ')) { kind = 'head'; r = r.slice(8); }
  else if (r.startsWith('tag: ')) { kind = 'tag'; r = r.slice(5); }
  if (r.startsWith('refs/heads/')) r = r.slice(11);
  else if (r.startsWith('refs/tags/')) r = r.slice(10);
  else if (r.startsWith('refs/remotes/')) { r = r.slice(13); if (kind === 'branch') kind = 'remote'; }
  return { label: r, kind };
}

const Refs = ({ refs }) => (refs?.length ? html`<span class="refs">${refs.map((r) => {
  const c = refChip(r);
  return html`<span class="ref" data-kind=${c.kind}>${c.label}</span>`;
})}</span>` : null);

function whenText(t) {
  const d = new Date(t * 1000);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

function CommitRow({ c, onOpen }) {
  return html`<button type="button" class="commit" data-testid="history-commit" data-oid=${c.id} onClick=${onOpen}>
    <span class="commit-oid">${c.id.slice(0, 7)}</span>
    <span class="commit-subject">${c.subject}</span>
    <${Refs} refs=${c.refs} />
    <span class="commit-meta">${c.author} · ${ago(c.time)}</span>
  </button>`;
}

function CommitView({ sid, oid, onBack }) {
  const [info, setInfo] = useState({ loading: true, data: null, error: null });
  const [file, setFile] = useState(null); // index | 'all' | null
  const [diff, setDiff] = useState(null);
  useEffect(() => {
    let dead = false;
    setInfo({ loading: true, data: null, error: null });
    setFile(null);
    reqQuiet('git.commit', { sid, oid })
      .then((data) => { if (!dead) setInfo({ loading: false, data, error: null }); })
      .catch((e) => { if (!dead) setInfo({ loading: false, data: null, error: e.message }); });
    return () => { dead = true; };
  }, [sid, oid]);
  useEffect(() => {
    if (file === null) { setDiff(null); return undefined; }
    let dead = false;
    setDiff({ loading: true });
    reqQuiet('git.diff', { sid, oid, ...(file === 'all' ? {} : { file }) })
      .then((r) => { if (!dead) setDiff({ loading: false, patch: r.patch ?? '', truncated: !!r.truncated }); })
      .catch((e) => { if (!dead) setDiff({ loading: false, error: e.message }); });
    return () => { dead = true; };
  }, [sid, oid, file]);
  const c = info.data;
  const [subject, ...rest] = (c?.message ?? '').split('\n');
  const body = rest.join('\n').trim();
  return html`<div class="commit-view">
    <div class="panel-bar">
      <button class="btn btn-sm" data-testid="history-back" onClick=${onBack}>← Log</button>
      <span class="panel-title"><code>${oid.slice(0, 12)}</code></span>
      ${c ? html`<button class="btn btn-sm" data-testid="commit-all" aria-pressed=${file === 'all'} onClick=${() => setFile('all')}>Whole commit</button>` : null}
    </div>
    ${info.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the commit: ${info.error}</p>`
      : !c ? html`<p class="panel-empty">Reading the commit…</p>`
      : html`<div class="commit-info">
        <h3 class="commit-title" data-testid="commit-subject">${subject}</h3>
        <p class="commit-by">${c.author} <span class="muted">${`<${c.email}>`}</span> · ${whenText(c.time)}</p>
        <${Refs} refs=${c.refs} />
        ${c.parents?.length ? html`<p class="commit-parents muted">parent${c.parents.length > 1 ? 's' : ''} ${c.parents.map((p) => p.slice(0, 7)).join(' ')}${c.first_parent ? ' · a merge: files are against the first parent' : ''}</p>` : null}
        ${body ? html`<pre class="commit-body">${body}</pre>` : null}
        <div class="commit-files">
          ${(c.files ?? []).map((f, i) => html`<button type="button" class="changes-file" data-testid="commit-file" data-path=${f.path} aria-pressed=${file === i} onClick=${() => setFile(i)}
            title=${f.old_path ? `${f.old_path} → ${f.path}` : f.path}>
            <span class="file-name">${f.old_path ? html`<span class="file-old">${f.old_path}</span> → ` : null}<b>${f.path}</b></span>
            ${f.added == null && f.removed == null ? html`<span class="file-counts muted">bin</span>` : html`<span class="file-counts"><span class="n-add">+${f.added ?? 0}</span> <span class="n-del">−${f.removed ?? 0}</span></span>`}
          </button>`)}
        </div>
      </div>`}
    ${file !== null ? html`<div class="diff-pane">
      <div class="diff-bar"><b class="diff-bar-path">${file === 'all' ? 'Whole commit' : c?.files?.[file]?.path ?? ''}</b><${ModeSwitch} />
        <button class="icon-btn" aria-label="Close the diff" title="Close the diff" onClick=${() => setFile(null)}>×</button></div>
      <div class="diff-scroll">
        ${diff?.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the diff: ${diff.error}</p>`
          : !diff || diff.loading ? html`<p class="panel-empty">Reading the diff…</p>`
          : html`<${DiffView} patch=${diff.patch} truncated=${diff.truncated} mode=${diffMode.value} />`}
      </div>
    </div>` : null}
  </div>`;
}

export function HistoryPanel({ sid }) {
  const [all, setAll] = useState(false);
  const [page, setPage] = useState({ loading: true, commits: [], more: false, head: null, error: null });
  const [open, setOpen] = useState(null);
  const [nonce, setNonce] = useState(0);
  const s = model.value.sessions[sid];
  const load = (skip) => reqQuiet('git.log', { sid, all, skip, limit: PAGE });
  useEffect(() => {
    let dead = false;
    setPage((p) => ({ ...p, loading: true, error: null }));
    load(0)
      .then((r) => { if (!dead) setPage({ loading: false, commits: r.commits ?? [], more: !!r.more, head: r.head ?? null, error: null }); })
      .catch((e) => { if (!dead) setPage({ loading: false, commits: [], more: false, head: null, error: e.message }); });
    return () => { dead = true; };
  }, [sid, all, nonce]);
  const more = () => {
    setPage((p) => ({ ...p, loading: true }));
    load(page.commits.length)
      .then((r) => setPage((p) => ({ loading: false, commits: [...p.commits, ...(r.commits ?? [])], more: !!r.more, head: p.head, error: null })))
      .catch((e) => setPage((p) => ({ ...p, loading: false, error: e.message })));
  };
  if (!s) return null;
  if (open) return html`<div class="history" data-testid="history" data-view="commit"><${CommitView} sid=${sid} oid=${open} onBack=${() => setOpen(null)} /></div>`;
  return html`<div class="history" data-testid="history" data-view="log">
    <div class="panel-bar">
      <span class="panel-title">${page.head ? html`⎇ ${page.head}` : 'Log'} <span class="count">${page.commits.length}${page.more ? '+' : ''}</span></span>
      <div class="seg" role="tablist" aria-label="Which branches">
        <button class="seg-btn" role="tab" data-testid="history-branch" aria-selected=${!all} onClick=${() => setAll(false)}>This branch</button>
        <button class="seg-btn" role="tab" data-testid="history-all" aria-selected=${all} onClick=${() => setAll(true)}>All</button>
      </div>
      <button class="icon-btn" title="Reload" aria-label="Reload the log" onClick=${() => setNonce(nonce + 1)}>↻</button>
    </div>
    <div class="history-list">
      ${page.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the log: ${page.error}</p>` : null}
      ${page.loading && !page.commits.length ? html`<p class="panel-empty">Reading the log…</p>` : null}
      ${!page.loading && !page.error && !page.commits.length ? html`<p class="panel-empty">No commits yet.</p>` : null}
      ${page.commits.map((c) => html`<${CommitRow} key=${c.id} c=${c} onOpen=${() => setOpen(c.id)} />`)}
      ${page.more ? html`<div class="history-more"><button class="btn btn-sm" data-testid="history-more" disabled=${page.loading} onClick=${more}>${page.loading ? 'Loading…' : `Load ${PAGE} more`}</button></div>` : null}
    </div>
  </div>`;
}

registerDrawerTab('history', { title: 'History', order: 30, Component: HistoryPanel });
defineAction('history', (sid) => toggleDrawer('history', typeof sid === 'string' ? sid : undefined));
