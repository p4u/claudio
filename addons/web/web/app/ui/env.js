// The Environment dialog (Alt+,): the env vars every claude session gets (Global) and the
// focused claude session's own (This session), applied at the session's next start or restart.
// Values are raw text (a newline is a newline). The server keeps them (`env.*`,
// docs/web-protocol.md §5); the dialog lists when it opens (or its connection does) and after
// each of its writes.
import { html } from 'htm/preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { model, focused, conn, reqQuiet } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog } from './modal.js';
import { nameNote, normalizeValue, scopeArgs, valueRows } from '../env.js';

// The last write still in flight: a list waits for it, so a dialog reopened right after an
// edit (Escape saves on blur) never lists the old value.
let writing = Promise.resolve();

export function installEnv() {
  register({ id: 'environment', group: 'App', title: 'Environment variables', keys: 'Alt+,', run: () => { overlay.value = 'env'; } });
}

function Row({ v, onSave, onDelete }) {
  const [text, setText] = useState(v.value);
  const field = useRef(null);
  // A list read again never overwrites what is being typed.
  useEffect(() => { if (document.activeElement !== field.current) setText(v.value); }, [v.value]);
  return html`<tr data-testid="env-row" data-name=${v.name}>
    <td class="env-name">${v.name}</td>
    <td><textarea ref=${field} class="env-value" data-testid="env-value" aria-label=${`${v.name} value`} rows=${valueRows(text)} value=${text}
      onInput=${(e) => setText(e.target.value)}
      onKeyDown=${(e) => { if (e.key === 'Escape') { if (e.currentTarget.value !== v.value) onSave(v.name, normalizeValue(e.currentTarget.value)); e.currentTarget.blur(); } }} onChange=${(e) => onSave(v.name, normalizeValue(e.target.value))}></textarea></td>
    <td><button type="button" class="btn btn-sm" data-testid="env-delete" aria-label=${`delete ${v.name}`} onClick=${() => onDelete(v.name)}>Delete</button></td>
  </tr>`;
}

export function EnvDialog() {
  const s = model.value.sessions[focused.value];
  const claude = s && s.kind === 'claude' ? s : null;
  const [tab, setTab] = useState('global');
  const [cat, setCat] = useState({ vars: [], refused: [] });
  const [list, setList] = useState({ loading: true, vars: [], skipped: 0 });
  const [error, setError] = useState(null);
  const [draft, setDraft] = useState({ name: '', value: '' });
  const shown = claude ? tab : 'global';
  const args = scopeArgs(shown, claude?.id);
  const scopeKey = JSON.stringify(args);
  const current = useRef(scopeKey);
  current.current = scopeKey;
  const open = conn.value === 'open';

  // A reply for a tab or session no longer shown is dropped (the replies come back in any order).
  const load = async () => {
    try {
      await writing;
      const r = await reqQuiet('env.list', args);
      if (current.current !== scopeKey) return;
      setList({ loading: false, vars: r.vars ?? [], skipped: r.skipped ?? 0 });
    } catch (e) {
      if (current.current !== scopeKey) return;
      setList({ loading: false, vars: [], skipped: 0 });
      setError(e.message);
    }
  };
  useEffect(() => {
    if (open) reqQuiet('env.catalogue').then(setCat).catch((e) => setError(e.message));
  }, [open]);
  useEffect(() => {
    if (!open) return;
    setError(null);
    setList((l) => ({ ...l, loading: true }));
    load();
  }, [open, shown, claude?.id]);

  // A write, then the list read again; the server's refusal is shown, never a value.
  const write = async (op, more) => {
    setError(null);
    try {
      const sent = reqQuiet(op, { ...args, ...more });
      writing = sent.catch(() => {});
      await sent;
    } catch (e) {
      if (current.current === scopeKey) setError(e.message);
      return false;
    }
    await load();
    return true;
  };
  const save = (name, value) => write('env.set', { name, value });
  const remove = (name) => write('env.unset', { name });
  const note = nameNote(cat, draft.name);
  const add = async () => {
    const name = draft.name.trim();
    if (note.bad) { setError(`${name}: not a variable name (letters, digits and _, not starting with a digit)`); return; }
    if (note.refused) { setError(`${name}: refused: ${note.refused}`); return; }
    if (await save(name, normalizeValue(draft.value))) setDraft({ name: '', value: '' });
  };

  return html`<${Dialog} title="Environment" testid="env" wide onClose=${() => { overlay.value = null; }}>
    <div class="ext-bar">
      <div class="seg" role="tablist">
        <button type="button" class="seg-btn" role="tab" data-testid="env-tab-global" aria-selected=${shown === 'global'} onClick=${() => setTab('global')}>Global</button>
        ${claude ? html`<button type="button" class="seg-btn" role="tab" data-testid="env-tab-session" aria-selected=${shown === 'session'} onClick=${() => setTab('session')}>This session: ${claude.label}</button>` : null}
      </div>
      <span class="muted">Applies on next start or restart. A changed value is saved when you leave its field.</span>
    </div>
    <p class="wz-note">${shown === 'global'
      ? 'Every claude session this machine launches, on every host. A var of the same name in a session wins; a proxy\'s wins over both.'
      : 'This session only, over the global vars.'} Values are stored here (0600) and reach claude's environment on the session's host.</p>
    ${error ? html`<p class="panel-empty panel-error" role="alert" data-testid="env-error">${error}</p>` : null}
    ${list.loading ? html`<p class="panel-empty">Reading…</p>` : null}
    ${list.skipped ? html`<p class="wz-note">${list.skipped} entr${list.skipped === 1 ? 'y' : 'ies'} of the file left out (not valid here).</p>` : null}
    <table class="env-table"><tbody>
      ${list.vars.map((v) => html`<${Row} key=${v.name} v=${v} onSave=${save} onDelete=${remove} />`)}
    </tbody></table>
    <div class="env-add">
      <input class="input" data-testid="env-add-name" list="env-names" placeholder="NAME" aria-label="name" autocomplete="off" spellcheck="false" data-autofocus
        value=${draft.name} onInput=${(e) => setDraft({ ...draft, name: e.target.value })} />
      <textarea class="env-value" data-testid="env-add-value" placeholder="value" aria-label="value" rows=${valueRows(draft.value)}
        value=${draft.value} onInput=${(e) => setDraft({ ...draft, value: e.target.value })}></textarea>
      <button type="button" class="btn btn-sm" data-testid="env-add" onClick=${add}>Add</button>
    </div>
    ${note.known ? html`<p class="env-desc" data-testid="env-add-desc"><span class="chip">${note.known.category}</span> ${note.known.description}</p>` : null}
    ${note.refused ? html`<p class="ext-refused">refused: ${note.refused}</p>` : null}
    <datalist id="env-names">${cat.vars.map((v) => html`<option key=${v.name} value=${v.name}>${v.category}: ${v.description}</option>`)}</datalist>
  <//>`;
}
