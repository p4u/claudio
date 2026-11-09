// The Changes tab (working tree status and diffs) and the shared diff view. Patch text is only ever
// rendered as text nodes: nothing from a patch becomes markup.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useRef, useState } from 'preact/hooks';
import { model, focused, selected, reqQuiet } from '../store.js';
import { defineAction } from '../palette.js';
import { parsePatch, sideBySide, groupWorktree } from '../diff.js';
import { registerDrawerTab, toggleDrawer } from './drawer.js';
import { toast } from './toasts.js';

const MODE_KEY = 'claudio.diffMode';
function loadMode() {
  try { return localStorage.getItem(MODE_KEY) === 'split' ? 'split' : 'unified'; } catch { return 'unified'; }
}
export const diffMode = signal(loadMode());
export function setDiffMode(m) {
  diffMode.value = m;
  try { localStorage.setItem(MODE_KEY, m); } catch { /* private mode */ }
}

const MARK = { add: '+', del: '−', ctx: ' ', meta: '' };

function UnifiedHunk({ h }) {
  return html`<div class="dhunk"><span>${h.header}</span></div>
    ${h.lines.map((l) => html`<div class="drow" data-kind=${l.kind}>
      <span class="dn dn-old">${l.old ?? ''}</span><span class="dn dn-new">${l.new ?? ''}</span><span class="dg">${MARK[l.kind]}</span><span class="dt">${l.text}</span>
    </div>`)}`;
}

// Side by side: each half is styled by its own line kind; a missing half is hatched.
function SplitRows({ h }) {
  return html`<div class="dhunk"><span>${h.header}</span></div>
    ${sideBySide(h).map((r) => {
      const half = (l, side) => (l
        ? html`<span class=${`dn dn-${side}`} data-kind=${l.kind}>${(side === 'old' ? l.old : l.new) ?? ''}</span><span class="dg" data-kind=${l.kind}>${MARK[l.kind]}</span><span class="dt" data-kind=${l.kind}>${l.text}</span>`
        : html`<span class="dn dempty"></span><span class="dg dempty"></span><span class="dt dempty"></span>`);
      return html`<div class="drow">${half(r.left, 'old')}${half(r.right, 'new')}</div>`;
    })}`;
}

function FileHead({ f }) {
  const name = f.newPath ?? f.oldPath ?? '(unknown)';
  const renamed = f.oldPath && f.newPath && f.oldPath !== f.newPath;
  let adds = 0;
  let dels = 0;
  for (const h of f.hunks) for (const l of h.lines) { if (l.kind === 'add') adds++; else if (l.kind === 'del') dels++; }
  return html`<header class="diff-file-head"><span class="diff-file-in">
    <b class="diff-path" title=${name}>${renamed ? `${f.oldPath} → ${name}` : name}</b>
    ${f.oldPath === null && f.newPath ? html`<span class="diff-tag diff-tag-new">new</span>` : null}
    ${f.newPath === null && f.oldPath ? html`<span class="diff-tag diff-tag-del">deleted</span>` : null}
    ${renamed ? html`<span class="diff-tag">renamed</span>` : null}
    ${f.binary ? html`<span class="diff-tag">binary</span>` : html`<span class="diff-counts"><span class="n-add">+${adds}</span> <span class="n-del">−${dels}</span></span>`}
  </span></header>`;
}

export function DiffView({ patch, truncated = false, mode = diffMode.value }) {
  const { files } = parsePatch(patch ?? '');
  const split = mode === 'split';
  return html`<div class="diff" data-testid="diff" data-mode=${mode}>
    ${truncated ? html`<p class="diff-notice" data-testid="diff-truncated" role="status">This patch was cut to fit the size limit. Open the file in an editor for the rest.</p>` : null}
    ${files.length === 0 ? html`<p class="diff-empty">No textual changes.</p>` : null}
    ${files.map((f, i) => html`<section class="diff-file" key=${i}>
      <${FileHead} f=${f} />
      ${f.binary ? html`<p class="diff-empty">Binary file: no preview.</p>`
        : f.hunks.length === 0 ? html`<p class="diff-empty">No textual changes (a rename or a mode change).</p>`
        : html`<div class=${`diff-grid ${split ? 'diff-split' : 'diff-unified'}`}>
          ${f.hunks.map((h, j) => (split ? html`<${SplitRows} key=${j} h=${h} />` : html`<${UnifiedHunk} key=${j} h=${h} />`))}
        </div>`}
    </section>`)}
  </div>`;
}

export function ModeSwitch() {
  const m = diffMode.value;
  return html`<div class="seg" role="tablist" aria-label="Diff layout">
    <button class="seg-btn" role="tab" data-testid="diff-unified" aria-selected=${m === 'unified'} onClick=${() => setDiffMode('unified')}>Unified</button>
    <button class="seg-btn" role="tab" data-testid="diff-split" aria-selected=${m === 'split'} onClick=${() => setDiffMode('split')}>Side by side</button>
  </div>`;
}

const LETTER_TONE = { A: 'add', '?': 'add', M: 'mod', T: 'mod', D: 'del', R: 'ren', C: 'ren', U: 'conflict' };

function Counts({ f, group }) {
  if (group === 'conflicts') return html`<span class="file-counts n-del">conflict</span>`;
  if (f.added == null && f.removed == null) return html`<span class="file-counts muted">${f.index === '?' ? 'new' : 'bin'}</span>`;
  return html`<span class="file-counts"><span class="n-add">+${f.added ?? 0}</span> <span class="n-del">−${f.removed ?? 0}</span></span>`;
}

function FileRow({ f, group, letter, active, onPick }) {
  const slash = f.path.lastIndexOf('/');
  const dir = slash >= 0 ? f.path.slice(0, slash + 1) : '';
  const base = slash >= 0 ? f.path.slice(slash + 1) : f.path;
  return html`<button type="button" class="changes-file" data-testid="changes-file" data-path=${f.path} data-group=${group} aria-pressed=${active} onClick=${onPick}
    title=${f.old_path ? `${f.old_path} → ${f.path}` : f.path}>
    <span class="file-letter" data-tone=${LETTER_TONE[letter] ?? 'mod'}>${letter}</span>
    <span class="file-name">${f.old_path ? html`<span class="file-old">${f.old_path}</span> → ` : null}<span class="file-dir">${dir}</span><b>${base}</b></span>
    <${Counts} f=${f} group=${group} />
  </button>`;
}

const GROUPS = [
  ['conflicts', 'Conflicts', () => 'U', false],
  ['staged', 'Staged', (f) => f.index, true],
  ['unstaged', 'Changes', (f) => f.worktree, false],
  ['untracked', 'Untracked', () => '?', false],
];

export const worktreeSupported = (s) => (model.value.hosts[s.host]?.features ?? []).includes('worktree');

export function ChangesPanel({ sid }) {
  const s = model.value.sessions[sid];
  const [st, setSt] = useState({ loading: true, data: null, error: null, rev: 0 });
  const [nonce, setNonce] = useState(0);
  const [sel, setSel] = useState(null); // {path, staged}
  const [diff, setDiff] = useState(null); // {loading, patch, truncated, error}
  const first = useRef(true);
  const gitKey = JSON.stringify(s?.git ?? null);
  const mode = diffMode.value;

  useEffect(() => {
    let dead = false;
    const run = () => reqQuiet('git.status', { sid })
      .then((data) => { if (!dead) setSt((p) => ({ loading: false, data, error: null, rev: p.rev + 1 })); })
      .catch((e) => { if (!dead) setSt((p) => ({ loading: false, data: null, error: e.message, rev: p.rev + 1 })); });
    const t = setTimeout(run, first.current ? 0 : 1000); // the session's git status changed: wait for it to settle
    first.current = false;
    return () => { dead = true; clearTimeout(t); };
  }, [sid, gitKey, nonce]);

  const files = st.data?.files ?? [];
  const groups = groupWorktree(files);
  const inBoth = sel ? groups.staged.some((f) => f.path === sel.path) && groups.unstaged.some((f) => f.path === sel.path) : false;
  const exists = sel ? files.some((f) => f.path === sel.path) : false;

  useEffect(() => {
    if (!sel) { setDiff(null); return undefined; }
    if (!exists) { setDiff({ loading: false, patch: '', truncated: false, error: null, gone: true }); return undefined; }
    let dead = false;
    setDiff((d) => ({ ...(d ?? {}), loading: true, error: null, gone: false }));
    reqQuiet('git.worktree_diff', { sid, path: sel.path, staged: sel.staged })
      .then((r) => { if (!dead) setDiff({ loading: false, patch: r.patch ?? '', truncated: !!r.truncated, error: null }); })
      .catch((e) => { if (!dead) setDiff({ loading: false, patch: '', truncated: false, error: e.message }); });
    return () => { dead = true; };
  }, [sid, sel?.path, sel?.staged, st.rev, exists]);

  if (!s) return null;
  const total = files.length;
  return html`<div class="changes" data-testid="changes" data-has-diff=${!!sel}>
    <div class="panel-bar">
      <span class="panel-title">${st.data?.branch ? html`⎇ ${st.data.branch}` : 'Working tree'} <span class="count">${total} file${total === 1 ? '' : 's'}</span></span>
      <button class="icon-btn" title="Reload" aria-label="Reload the changes" onClick=${() => setNonce(nonce + 1)}>↻</button>
    </div>
    <div class="changes-list">
      ${st.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the changes: ${st.error}</p>`
        : st.loading && !st.data ? html`<p class="panel-empty">Reading the working tree…</p>`
        : total === 0 ? html`<p class="panel-empty">The working tree is clean.</p>` : null}
      ${st.data?.truncated ? html`<p class="panel-note" role="status">The list is cut: there are more changed files than this view shows.</p>` : null}
      ${GROUPS.map(([key, title, letterOf, staged]) => groups[key].length === 0 ? null : html`<section class="changes-group" key=${key} data-testid="changes-group" data-group=${key}>
        <h3>${title} <span class="count">${groups[key].length}</span></h3>
        ${groups[key].map((f) => html`<${FileRow} key=${`${key}:${f.path}`} f=${f} group=${key} letter=${letterOf(f)}
          active=${sel?.path === f.path && sel.staged === staged}
          onPick=${() => setSel({ path: f.path, staged })} />`)}
      </section>`)}
    </div>
    ${sel ? html`<div class="diff-pane">
      <div class="diff-bar">
        <span class="diff-bar-path muted">${sel.staged ? 'Staged' : 'Working tree'} · ${sel.path.slice(sel.path.lastIndexOf('/') + 1)}</span>
        ${inBoth ? html`<div class="seg" role="tablist" aria-label="Which version">
          <button class="seg-btn" role="tab" data-testid="diff-unstaged" aria-selected=${!sel.staged} onClick=${() => setSel({ path: sel.path, staged: false })}>Unstaged</button>
          <button class="seg-btn" role="tab" data-testid="diff-staged" aria-selected=${sel.staged} onClick=${() => setSel({ path: sel.path, staged: true })}>Staged</button>
        </div>` : html`<span class="chip">${sel.staged ? 'staged' : 'unstaged'}</span>`}
        <${ModeSwitch} />
        <button class="icon-btn" aria-label="Close the diff" title="Close the diff" onClick=${() => setSel(null)}>×</button>
      </div>
      <div class="diff-scroll">
        ${diff?.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the diff: ${diff.error}</p>`
          : diff?.gone ? html`<p class="panel-empty">This file is no longer changed.</p>`
          : !diff || (diff.loading && !diff.patch) ? html`<p class="panel-empty">Reading the diff…</p>`
          : html`<${DiffView} patch=${diff.patch} truncated=${diff.truncated} mode=${mode} />`}
      </div>
    </div>` : null}
  </div>`;
}

const noWorktree = (s) => (worktreeSupported(s) ? null : "This host's daemon is too old to list changes. Run `claudio daemon restart` there.");

registerDrawerTab('changes', { title: 'Changes', order: 20, Component: ChangesPanel, disabled: noWorktree });
defineAction('changes', (sid) => {
  const id = typeof sid === 'string' ? sid : (focused.value ?? selected.value);
  const s = id ? model.value.sessions[id] : null;
  if (!s) return;
  const why = noWorktree(s);
  if (why) { toast({ text: why, level: 'warn' }); return; }
  toggleDrawer('changes', id);
});
