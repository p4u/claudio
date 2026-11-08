// The new-session wizard: one dialog, top to bottom (host, kind, folder, conversation, proxy, name, prompt).
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { model, focused, reqQuiet } from '../store.js';
import { overlay, defineAction } from '../palette.js';
import { termSize } from '../term.js';
import { initialWizard, proxyOptions, firstPartyAllowed, resolvedProfile, spawnRequest, filterDirs } from '../wizard.js';
import { beforeSpawn } from '../spawn-hooks.js';
import { Dialog } from './modal.js';
import { toast } from './toasts.js';
import { ago } from './glyph.js';
import { openSession } from './session-actions.js';

export const wizardArgs = signal({});

// openWizard({ host, cwd, resume, kind }): all optional.
export function openWizard(opts = {}) {
  wizardArgs.value = { ...opts };
  overlay.value = 'wizard';
}

const abs = (p) => typeof p === 'string' && p.startsWith('/');
const dirname = (p) => { const t = p.replace(/\/+$/, ''); const i = t.lastIndexOf('/'); return i <= 0 ? '/' : t.slice(0, i); };
const join = (a, b) => `${a.replace(/\/+$/, '')}/${b}`;

function useDebounced(value, ms) {
  const [v, setV] = useState(value);
  useEffect(() => { const t = setTimeout(() => setV(value), ms); return () => clearTimeout(t); }, [value, ms]);
  return v;
}

function HostField({ w, set }) {
  const m = model.value;
  const connected = Object.values(m.hosts).filter((h) => h.connected).map((h) => h.name);
  const [cands, setCands] = useState([]);
  const [busy, setBusy] = useState(null);
  const [err, setErr] = useState(null);
  useEffect(() => { reqQuiet('hosts.candidates').then((r) => setCands((r.hosts ?? []).filter((h) => !connected.includes(h)))).catch(() => {}); }, []);
  async function pick(v) {
    setErr(null);
    if (!v.startsWith('ssh:')) { set({ host: v, resume: null }); return; }
    const name = v.slice(4);
    setBusy(name);
    try {
      await reqQuiet('hosts.connect', { host: name });
      set({ host: name, hosts: [...new Set([...w.hosts, name])], resume: null });
    } catch (e) {
      setErr(e.message);
    }
    setBusy(null);
  }
  return html`<div class="wz-field">
    <label for="wz-host">Host</label>
    <select id="wz-host" class="select" data-testid="wizard-host" value=${w.host} disabled=${busy !== null} onChange=${(e) => pick(e.target.value)}>
      ${connected.map((h) => html`<option value=${h} selected=${h === w.host}>${h === 'local' ? 'local (this machine)' : h}</option>`)}
      ${w.host && !connected.includes(w.host) ? html`<option value=${w.host} selected>${w.host}</option>` : null}
      ${cands.length ? html`<optgroup label="Connect over SSH">${cands.map((c) => html`<option value=${`ssh:${c}`}>${c}</option>`)}</optgroup>` : null}
    </select>
    ${busy ? html`<p class="wz-note">Connecting to ${busy}… (first time it uploads claudio)</p>` : null}
    ${err ? html`<pre class="wz-error" role="alert" data-testid="wizard-host-error">${err}</pre>` : null}
  </div>`;
}

function Folders({ w, set }) {
  const [dirs, setDirs] = useState(null);
  const [browse, setBrowse] = useState(null); // { path, entries, truncated, error }
  const [showHidden, setShowHidden] = useState(false);
  useEffect(() => {
    let dead = false;
    setDirs(null);
    reqQuiet('fs.projects', { host: w.host, limit: 50 }).then((r) => { if (!dead) setDirs(r.dirs ?? []); }).catch(() => { if (!dead) setDirs([]); });
    return () => { dead = true; };
  }, [w.host]);
  const list = useMemo(() => filterDirs(dirs ?? [], w.cwd).slice(0, 8), [dirs, w.cwd]);
  async function open(path) {
    setBrowse({ path, entries: null, error: null });
    try {
      const r = await reqQuiet('fs.list', { host: w.host, path });
      setBrowse({ path: r.path ?? path, entries: r.entries ?? [], truncated: !!r.truncated, error: null });
    } catch (e) {
      setBrowse({ path, entries: [], error: e.message });
    }
  }
  const home = dirs?.[0] ? dirname(dirs[0].path) : '/';
  return html`<div class="wz-field">
    <label for="wz-dir">Folder</label>
    <input id="wz-dir" class="input" data-testid="wizard-dir-input" data-autofocus value=${w.cwd} placeholder="/path/to/project" autocomplete="off" spellcheck=${false}
      onInput=${(e) => set({ cwd: e.target.value, resume: null })} />
    ${w.cwd && !abs(w.cwd) ? html`<p class="wz-note wz-warn">Type an absolute path (it starts with /).</p>` : null}
    <ul class="wz-dirs" data-testid="wizard-dirs">
      ${dirs === null ? html`<li class="wz-note">Looking for recent projects…</li>` : null}
      ${list.map((d) => html`<li key=${d.path}><button type="button" class="wz-dir" data-testid="wizard-dir" data-path=${d.path} aria-pressed=${d.path === w.cwd} onClick=${() => set({ cwd: d.path, resume: null })}>
        <span class="wz-dir-path">${d.typed ? `Use ${d.path}` : d.path}</span>
        ${d.git ? html`<span class="wz-dir-branch">⎇ ${d.git}</span>` : null}
        ${d.modified ? html`<span class="wz-dir-age">${ago(d.modified)}</span>` : null}
      </button></li>`)}
    </ul>
    <button type="button" class="wz-link" data-testid="wizard-browse" aria-expanded=${browse !== null} onClick=${() => (browse ? setBrowse(null) : open(abs(w.cwd) ? w.cwd : home))}>
      ${browse ? 'Hide folder browser' : 'Browse folders…'}</button>
    ${browse ? html`<div class="wz-browser" data-testid="wizard-browser">
      <div class="wz-browser-head">
        <button type="button" class="btn btn-sm" aria-label="Parent folder" disabled=${browse.path === '/'} onClick=${() => open(dirname(browse.path))}>..</button>
        <code class="wz-browser-path" title=${browse.path}>${browse.path}</code>
        <button type="button" class="btn btn-sm" onClick=${() => set({ cwd: browse.path, resume: null })}>Use this folder</button>
        <label class="wz-check"><input type="checkbox" checked=${showHidden} onChange=${(e) => setShowHidden(e.target.checked)} /> hidden</label>
      </div>
      <ul class="wz-entries">
        ${browse.entries === null ? html`<li class="wz-note">Reading…</li>` : null}
        ${browse.error ? html`<li class="wz-note wz-warn">${browse.error}</li>` : null}
        ${(browse.entries ?? []).filter((e) => e.dir && (showHidden || !e.hidden)).map((e) => html`<li key=${e.name}>
          <button type="button" class="wz-entry" onClick=${() => { set({ cwd: join(browse.path, e.name), resume: null }); open(join(browse.path, e.name)); }}>
            <span>▸ ${e.name}${e.symlink ? ' →' : ''}</span>
            ${e.git ? html`<span class="wz-dir-branch">⎇ ${e.git}</span>` : null}
            ${e.claude_at ? html`<span class="wz-dir-age">claude ${ago(e.claude_at)}</span>` : null}
          </button></li>`)}
        ${browse.truncated ? html`<li class="wz-note">The list is cut. Type the path to go further.</li>` : null}
      </ul>
    </div>` : null}
  </div>`;
}

function ResumeField({ w, set }) {
  const cwd = useDebounced(w.cwd, 300);
  const [list, setList] = useState(null);
  useEffect(() => {
    if (!abs(cwd)) { setList(null); return; }
    let dead = false;
    reqQuiet('claude.sessions', { host: w.host, cwd }).then((r) => { if (!dead) setList(r.sessions ?? []); }).catch(() => { if (!dead) setList([]); });
    return () => { dead = true; };
  }, [w.host, cwd]);
  const known = list?.some((x) => x.id === w.resume);
  if (!w.resume && !(list && list.length)) return null;
  return html`<div class="wz-field" data-testid="wizard-resume">
    <label>Conversation</label>
    <ul class="wz-resume" role="radiogroup" aria-label="Conversation">
      <li><label class="wz-radio"><input type="radio" name="wz-resume" checked=${!w.resume} onChange=${() => set({ resume: null })} /> <span><b>New conversation</b></span></label></li>
      ${w.resume && !known ? html`<li><label class="wz-radio"><input type="radio" name="wz-resume" checked onChange=${() => {}} /> <span><b>Resume ${w.resume.slice(0, 8)}</b></span></label></li>` : null}
      ${(list ?? []).map((x) => html`<li key=${x.id}><label class="wz-radio" data-testid="wizard-resume-item" data-id=${x.id}>
        <input type="radio" name="wz-resume" checked=${w.resume === x.id} onChange=${() => set({ resume: x.id })} />
        <span><b>${x.title || x.last_prompt || x.id.slice(0, 8)}</b>
          <small>${x.messages} messages · ${ago(x.modified)}${x.last_prompt && x.title ? ` · ${x.last_prompt}` : ''}</small></span>
      </label></li>`)}
    </ul>
  </div>`;
}

export function Wizard() {
  const args = wizardArgs.value;
  const m = model.value;
  const [w, setW] = useState(() => initialWizard({
    hosts: Object.values(m.hosts).filter((h) => h.connected).map((h) => h.name),
    profiles: m.profiles, recent: [], focusedHost: args.host ?? (focused.value ? m.sessions[focused.value]?.host : 'local'),
    kind: args.kind ?? 'claude', cwd: args.cwd ?? '', resume: args.resume ?? null,
  }));
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const set = (patch) => setW((cur) => ({ ...cur, ...patch }));
  const profiles = m.profiles;
  const shell = w.kind === 'shell';
  const ready = abs(w.cwd) && !busy;
  const fpOk = firstPartyAllowed(w.proxy, profiles);

  async function start() {
    if (!ready || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      let skip = [];
      if (!shell) {
        const r = await beforeSpawn(resolvedProfile(w.proxy, profiles));
        if (r === null) return;
        skip = r.skip;
      }
      const size = termSize(focused.value) ?? { rows: 24, cols: 80 };
      const { op, args: a } = spawnRequest({ ...w, profiles, skip }, size);
      const { sid } = await reqQuiet(op, a);
      overlay.value = null;
      for (let i = 0; i < 20 && !model.value.sessions[sid]; i++) await new Promise((r) => setTimeout(r, 100));
      if (model.value.sessions[sid]) openSession(sid);
    } catch (e) {
      toast({ text: `${shell ? 'terminal' : 'session'}: ${e.message}`, level: 'error' });
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  const key = (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey || (e.target.tagName === 'INPUT' && e.target.type === 'text'))) { e.preventDefault(); start(); }
  };

  return html`<${Dialog} title=${shell ? 'New terminal' : 'New session'} testid="wizard" wide onClose=${() => { overlay.value = null; }}>
    <div class="wizard" onKeyDown=${key}>
      <${HostField} w=${w} set=${set} />
      <div class="wz-field">
        <label id="wz-kind-l">What to start</label>
        <div class="seg" role="radiogroup" aria-labelledby="wz-kind-l" data-testid="wizard-kind" data-value=${w.kind}>
          <button type="button" class="seg-btn" role="radio" aria-checked=${!shell} data-testid="wizard-kind-claude" onClick=${() => set({ kind: 'claude' })}>Claude</button>
          <button type="button" class="seg-btn" role="radio" aria-checked=${shell} data-testid="wizard-kind-shell" onClick=${() => set({ kind: 'shell', resume: null })}>Terminal</button>
        </div>
      </div>
      <${Folders} w=${w} set=${set} />
      ${shell ? null : html`<${ResumeField} w=${w} set=${set} />
        <div class="wz-field">
          <label for="wz-proxy">Proxy</label>
          <select id="wz-proxy" class="select" data-testid="wizard-proxy" value=${w.proxy} onChange=${(e) => set({ proxy: e.target.value, firstParty: false })}>
            ${proxyOptions(profiles).map((o) => html`<option value=${o.value} selected=${o.value === w.proxy}>${o.label}</option>`)}
          </select>
          <label class="wz-check"><input type="checkbox" data-testid="wizard-first-party" checked=${w.firstParty && fpOk} disabled=${!fpOk} onChange=${(e) => set({ firstParty: e.target.checked })} />
            First-party session${fpOk ? '' : ' (needs a proxy profile)'}</label>
        </div>`}
      <div class="wz-field">
        <label for="wz-name">Name <span class="muted">(optional)</span></label>
        <input id="wz-name" class="input" data-testid="wizard-name" value=${w.name} maxlength="200" onInput=${(e) => set({ name: e.target.value })} />
      </div>
      ${shell ? null : html`<div class="wz-field">
        <label for="wz-prompt">Opening prompt <span class="muted">(optional, pasted once claude is ready)</span></label>
        <textarea id="wz-prompt" class="input wz-prompt" data-testid="wizard-prompt" rows="3" value=${w.prompt} onInput=${(e) => set({ prompt: e.target.value })}></textarea>
      </div>`}
    </div>
    <div class="dialog-actions">
      <button type="button" class="btn" onClick=${() => { overlay.value = null; }}>Cancel</button>
      <button type="button" class="btn btn-primary" data-testid="wizard-start" disabled=${!ready} onClick=${start}>${busy ? 'Starting…' : shell ? 'Open terminal' : 'Start session'}</button>
    </div>
  <//>`;
}

export function installWizard() {
  defineAction('new-session', () => openWizard({ kind: 'claude' }));
  defineAction('new-terminal', () => openWizard({ kind: 'shell' }));
}
