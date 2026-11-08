// The host panel: facts about one host, connect/disconnect, add a host, claude install/update.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { model, req, reqQuiet } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog, confirm } from './modal.js';
import { toast } from './toasts.js';
import { updateOffered } from './claude.js';

export const hostPanel = signal(''); // the host shown ('' = just the add form)

export function openHostPanel(name = '') {
  hostPanel.value = name;
  overlay.value = 'hosts';
}

export function installHosts() {
  register({ id: 'hosts', group: 'App', title: 'Hosts', run: () => openHostPanel(model.value.hosts.local ? 'local' : '') });
  register({ id: 'host-add', group: 'App', title: 'Add a host', run: () => openHostPanel('') });
}


// A tiny horizontal bar: the fill width is the only inline style.
export function MiniBar({ value, label }) {
  const v = Math.min(100, Math.max(0, Math.round(value)));
  return html`<span class="mini" data-level=${v >= 85 ? 'hot' : v >= 60 ? 'warn' : 'ok'} title=${`${label} ${v}%`}><i style=${{ width: `${v}%` }}></i></span>`;
}

export function hostGlyph(h) {
  if (h.connecting) return '⇄';
  return '';
}

function Fact({ k, v, mono }) {
  if (v === null || v === undefined || v === '') return null;
  return html`<div class="kv"><dt>${k}</dt><dd class=${mono ? 'mono' : ''}>${v}</dd></div>`;
}

function HostFacts({ h }) {
  const claude = model.value.claude[h.name];
  const working = claude && (claude.phase === 'installing' || claude.phase === 'updating');
  const offered = updateOffered(claude) ? claude : null;
  const remote = h.name !== 'local';
  const state = h.connected ? 'connected' : h.connecting ? 'connecting…' : 'offline';
  const sessions = Object.values(model.value.sessions).filter((s) => s.host === h.name).length;
  async function disconnect() {
    const ok = await confirm({ title: `Disconnect ${h.name}?`, body: `${sessions} session${sessions === 1 ? '' : 's'} on it leave this list. They keep running on the host and come back when you connect again.`, yes: 'Disconnect', no: 'Stay' });
    if (ok) req('hosts.disconnect', { host: h.name }).then(() => { hostPanel.value = ''; }).catch(() => {});
  }
  return html`<div class="host-facts" data-testid="host-panel-facts" data-host=${h.name}>
    <dl class="kv-list">
      <${Fact} k="Connection" v=${state} />
      <${Fact} k="Hostname" v=${h.hostname} />
      <${Fact} k="System" v=${h.os ? `${h.os} ${h.arch ?? ''}`.trim() : null} />
      <${Fact} k="claude" v=${h.claude ? `${h.claude.version}` : (h.connected ? 'not installed' : null)} />
      <${Fact} k="claude path" v=${h.claude?.path} mono />
      ${offered ? html`<${Fact} k="Update" v=${`claude ${offered.have ?? '?'} → ${offered.want ?? '?'} is available`} />` : null}
      <${Fact} k="Daemon" v=${h.version ? `claudio ${h.version}${h.outdated ? ' (older than this server; it is replaced once idle)' : ''}` : null} />
      <${Fact} k="Sessions" v=${sessions} />
      <div class="kv"><dt>Features</dt><dd>${(h.features ?? []).length ? (h.features ?? []).map((f) => html`<span class="chip" key=${f}>${f}</span>`) : html`<span class="muted">none</span>`}</dd></div>
    </dl>
    ${h.error ? html`<pre class="host-error" data-testid="host-error">${h.error}</pre>` : null}
    <div class="host-actions">
      ${h.connected ? html`<button type="button" class=${`btn btn-sm${offered ? ' btn-primary' : ''}`} data-testid="host-claude-btn" data-autofocus=${offered ? true : null} disabled=${working} onClick=${() => req('claude.update', { host: h.name, install: !h.claude }).catch(() => {})}>${working ? 'Working…' : h.claude ? 'Update claude' : 'Install claude'}</button>` : null}
      ${offered ? html`<button type="button" class="btn btn-sm" data-testid="host-claude-skip" title="Do not offer this version again" onClick=${() => req('claude.skip', { host: h.name }).catch(() => {})}>Skip this version</button>` : null}
      ${remote && !h.connected ? html`<button type="button" class="btn btn-sm btn-primary" data-testid="host-connect-btn" disabled=${h.connecting} onClick=${() => req('hosts.connect', { host: h.name }).catch(() => {})}>${h.connecting ? 'Connecting…' : 'Connect'}</button>` : null}
      ${remote && h.connected ? html`<button type="button" class="btn btn-sm btn-deny" data-testid="host-disconnect-btn" onClick=${disconnect}>Disconnect</button>` : null}
    </div>
  </div>`;
}

function AddHost() {
  const [alias, setAlias] = useState('');
  const [cands, setCands] = useState([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState(null);
  useEffect(() => { reqQuiet('hosts.candidates').then((r) => setCands(r.hosts ?? [])).catch(() => {}); }, []);
  async function connect(name) {
    const host = name.trim();
    if (!host) return;
    setBusy(true); setErr(null);
    try {
      await reqQuiet('hosts.connect', { host });
      toast({ text: `Connected to ${host}.`, level: 'info' });
      setAlias('');
      hostPanel.value = host;
    } catch (e) {
      setErr(e.message);
    } finally { setBusy(false); }
  }
  return html`<section class="host-add" data-testid="host-add">
    <h3>Add a host</h3>
    <p class="wz-note">An ssh alias from <code>~/.ssh/config</code>, or user@host. claudio installs itself there if needed.</p>
    <div class="host-add-row">
      <input class="input" data-testid="host-add-input" list="host-candidates" placeholder="alias or user@host" autocomplete="off" spellcheck=${false} value=${alias}
        onInput=${(e) => setAlias(e.target.value)} onKeyDown=${(e) => { if (e.key === 'Enter') { e.preventDefault(); connect(alias); } }} />
      <datalist id="host-candidates">${cands.map((c) => html`<option value=${c}></option>`)}</datalist>
      <button type="button" class="btn btn-primary" data-testid="host-add-btn" disabled=${busy || !alias.trim()} onClick=${() => connect(alias)}>${busy ? 'Connecting…' : 'Connect'}</button>
    </div>
    ${cands.length ? html`<div class="host-cands">${cands.slice(0, 12).map((c) => html`<button type="button" class="chip chip-btn" key=${c} data-testid="host-candidate" onClick=${() => setAlias(c)}>${c}</button>`)}</div>` : null}
    ${err ? html`<pre class="host-error" data-testid="host-connect-error" role="alert">${err}</pre>` : null}
  </section>`;
}

export function HostPanel() {
  const m = model.value;
  const names = Object.keys(m.hosts).sort((a, b) => (a === 'local' ? -1 : b === 'local' ? 1 : a.localeCompare(b)));
  const cur = hostPanel.value && m.hosts[hostPanel.value] ? m.hosts[hostPanel.value] : null;
  return html`<${Dialog} title=${cur ? `Host ${cur.name}` : 'Hosts'} testid="host-panel" wide onClose=${() => { overlay.value = null; }}>
    <div class="host-tabs" role="tablist">${names.map((n) => html`<button type="button" class="seg-btn host-tab" role="tab" key=${n} aria-selected=${cur?.name === n} onClick=${() => { hostPanel.value = n; }}>${n}</button>`)}
      <button type="button" class="seg-btn host-tab" role="tab" aria-selected=${!cur} onClick=${() => { hostPanel.value = ''; }}>+ add</button></div>
    ${cur ? html`<${HostFacts} h=${cur} />` : null}
    ${cur ? null : html`<${AddHost} />`}
  <//>`;
}
