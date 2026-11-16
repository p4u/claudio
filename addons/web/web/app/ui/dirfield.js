// The wizard's folder field: a combobox that navigates the host's folders as you type (a path
// starting with / or ~ lists its parent's subfolders), or offers the recent folders otherwise.
// The pure rules are in ../dirnav.js; this is the state, the keys and the drawing.
import { html } from 'htm/preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { reqQuiet } from '../store.js';
import { isPath, splitPath, folderRows, useRow, firstHighlight, complete, backspaceUp, recentRows, sequencer, listNote } from '../dirnav.js';
import { ago } from './glyph.js';

const LIST_DEBOUNCE = 120;

// `w.cwd` is the field's text; `home` the host's home (null until known) for `~`; `online` whether
// the connection is up (requests wait for it). `browse` is bumped by "Browse folders…" (it sets
// the text to `~/` and opens the list). `onPicked` moves the focus on to Start; `onEdit` (any edit
// of the field) cancels that move if it is still waiting.
export function DirField({ w, set, home, online, browse, onPicked, onEdit }) {
  const [dirs, setDirs] = useState(null);
  const [listing, setListing] = useState(null); // { parent, entries | null, truncated, error }
  const [open, setOpen] = useState(false);
  const [sel, setSel] = useState(0);
  const [showHidden, setShowHidden] = useState(false);
  const seq = useRef(sequencer());
  const input = useRef(null);
  const list = useRef(null);
  const text = w.cwd;
  const path = isPath(text);
  const { parent, fragment } = splitPath(text);

  // A host change makes its listing stale and closes the list; the connection coming up does not
  // (the person may have typed before it did).
  useEffect(() => { seq.current.drop(); setListing(null); setOpen(false); }, [w.host]);
  useEffect(() => {
    let dead = false;
    setDirs(null);
    if (!online) return undefined;
    reqQuiet('fs.projects', { host: w.host, limit: 50 }).then((r) => { if (!dead) setDirs(r.dirs ?? []); }).catch(() => { if (!dead) setDirs([]); });
    return () => { dead = true; };
  }, [w.host, online]);

  // "Browse folders…": the list at ~/.
  useEffect(() => {
    if (!browse) return;
    set({ cwd: '~/', resume: null });
    setOpen(true);
    input.current?.focus();
  }, [browse]);

  // The parent's listing, asked when the parent changes (debounced); only the latest reply counts.
  const want = open && online && path && parent ? parent : null;
  useEffect(() => {
    if (!want || listing?.parent === want) return undefined;
    const t = setTimeout(() => {
      const mine = seq.current.next();
      setListing({ parent: want, entries: null, truncated: false, error: null });
      reqQuiet('fs.list', { host: w.host, path: want })
        .then((r) => { if (seq.current.fresh(mine)) setListing({ parent: want, entries: r.entries ?? [], truncated: !!r.truncated, error: null }); })
        .catch((e) => { if (seq.current.fresh(mine)) setListing({ parent: want, entries: [], truncated: false, error: e.message }); });
    }, LIST_DEBOUNCE);
    return () => clearTimeout(t);
  }, [want, w.host]);

  const listed = listing?.parent === parent ? listing : null;
  const folders = useMemo(() => (path && listed ? folderRows(listed.entries, fragment, showHidden) : []), [path, listed, fragment, showHidden]);
  const recents = useMemo(() => (path ? [] : recentRows(dirs ?? [], text)), [path, dirs, text]);
  const use = path ? useRow(text, listed) : null;
  // Rows: path mode [use, ...folders], else the recent folders.
  const count = path ? 1 + folders.length : recents.length;
  // The highlight as drawn: `sel` is reset after paint, so a key before that sees it clamped.
  const cur = Math.min(sel, Math.max(0, count - 1));
  useEffect(() => { setSel(path ? firstHighlight(fragment, folders) : 0); }, [text, listed, showHidden]);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [sel, open]);

  const edit = (cwd) => { set({ cwd, resume: null }); setOpen(true); onEdit?.(); };
  const pick = (cwd) => { set({ cwd, resume: null }); setOpen(false); onPicked(); };
  // The highlighted row's folder to complete into the field (null on "use").
  const folderAt = (i) => (path ? (i >= 1 && folders[i - 1] ? complete(parent, folders[i - 1]) : null) : recents[i] ? `${recents[i].path.replace(/\/+$/, '')}/` : null);
  const choose = (i) => {
    if (path && i === 0) { if (!use.notFound) pick(use.target); return; }
    if (path) { const next = folderAt(i); if (next) edit(next); return; }
    if (recents[i]) pick(recents[i].path);
  };
  const atEnd = (e) => e.target.selectionStart === e.target.value.length && e.target.selectionEnd === e.target.value.length;

  const key = (e) => {
    if (!open) {
      if (e.key === 'ArrowDown') { e.preventDefault(); setOpen(true); }
      return; // Enter starts the session (the wizard's handler); Esc closes the dialog.
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setSel(Math.max(0, Math.min(count - 1, cur + (e.key === 'ArrowDown' ? 1 : -1))));
    } else if ((e.key === 'Tab' && !e.shiftKey) || (e.key === 'ArrowRight' && atEnd(e))) {
      const next = folderAt(cur);
      if (next) { e.preventDefault(); edit(next); }
    } else if (e.key === 'Backspace' && atEnd(e)) {
      const up = backspaceUp(text, home);
      if (up !== null) { e.preventDefault(); if (up !== text) edit(up); }
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      if (count) choose(cur);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  const noteText = listNote(listed);
  const note = noteText ? html`<p class=${`wz-note${listed.error ? ' wz-warn' : ''}`} data-testid="wizard-dir-error" role=${listed.error ? 'alert' : null}>${noteText}</p>` : null;
  return html`<div class="wz-combo">
    <input id="wz-dir" ref=${input} class="input" data-testid="wizard-dir-input" data-autofocus value=${text} placeholder="~/ or /path/to/project" autocomplete="off" spellcheck=${false}
      role="combobox" aria-expanded=${open} aria-controls="wz-dir-list" aria-autocomplete="list"
      onInput=${(e) => edit(e.target.value)} onKeyDown=${key} onBlur=${() => setOpen(false)} />
    ${open ? html`<ul id="wz-dir-list" class="wz-dir-list" role="listbox" data-testid="wizard-dir-list" ref=${list} onMouseDown=${(e) => e.preventDefault()}>
      ${path ? html`<li role="option" class="wz-dir-row wz-dir-use" data-testid="wizard-dir-use" aria-selected=${cur === 0} aria-disabled=${use.notFound}
          onMouseMove=${() => cur !== 0 && setSel(0)} onClick=${() => choose(0)}>
          <span class="wz-dir-path">▸ use ${use.target}${use.notFound ? ' (not found)' : ''}</span>
        </li>` : null}
      ${path && listed?.entries === null ? html`<li class="wz-note">Reading…</li>` : null}
      ${path ? folders.map((name, k) => html`<li key=${name} role="option" class="wz-dir-row" data-testid="wizard-dir-row" data-path=${`${parent}${name}`} aria-selected=${cur === k + 1}
          onMouseMove=${() => cur !== k + 1 && setSel(k + 1)} onClick=${() => choose(k + 1)}>
          <span class="wz-dir-path">${name}/</span>
        </li>`) : null}
      ${!path && dirs === null ? html`<li class="wz-note">Looking for recent projects…</li>` : null}
      ${!path ? recents.map((d, k) => html`<li key=${d.path} role="option" class="wz-dir-row" data-testid="wizard-dir-row" data-path=${d.path} aria-selected=${cur === k}
          onMouseMove=${() => cur !== k && setSel(k)} onClick=${() => choose(k)}>
          <span class="wz-dir-path">${d.path}</span>
          ${d.git ? html`<span class="wz-dir-branch">⎇ ${d.git}</span>` : null}
          ${d.modified ? html`<span class="wz-dir-age">${ago(d.modified)}</span>` : null}
        </li>`) : null}
    </ul>` : null}
    ${open && path ? html`<div class="wz-dir-tools" onMouseDown=${(e) => e.preventDefault()}>
      ${note ?? html`<span></span>`}
      <label class="wz-check"><input type="checkbox" tabindex="-1" checked=${showHidden} onChange=${(e) => setShowHidden(e.target.checked)} /> hidden</label>
    </div>` : null}
  </div>`;
}
