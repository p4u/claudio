// The extensions screen: built-ins, external extensions and proxy plugins, with toggles, and for
// plugins the install status per host (with the live retry countdown) and their setup.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { model, req, reqQuiet, onServerEvent } from '../store.js';
import { applyEvent } from '../model.js';
import { overlay, register } from '../palette.js';
import { Dialog } from './modal.js';
import { openPluginSetup } from './plugin-setup.js';

const reload = signal(0); // bumped by extensions_changed
const stamped = new WeakMap(); // plugin status object -> when this page learnt it
const reasked = new WeakSet(); // skipped items whose due retry already triggered a refresh

function stamp(key) {
  const st = model.value.plugins[key];
  if (st && typeof st === 'object' && !stamped.has(st)) stamped.set(st, Date.now());
}

export function installExtensions() {
  onServerEvent((ev) => {
    if (ev.ev === 'extensions_changed') reload.value++;
    else if (ev.ev === 'plugin_status') stamp(`${ev.host}/${ev.name}`);
  });
  register({ id: 'extensions', group: 'App', title: 'Extensions', run: () => { overlay.value = 'extensions'; } });
}

// "failed: network (retrying in 30 s)" -> "failed: network": the countdown is drawn live.
export const bareReason = (r) => String(r ?? '').replace(/\s*\(retrying in \d+ s\)\s*$/, '');

export function retryLeft(st, item, nowMs) {
  if (item.retry_secs == null) return null;
  const at = stamped.get(st) ?? nowMs;
  return Math.max(0, item.retry_secs - Math.floor((nowMs - at) / 1000));
}

const SOURCE = { builtin: 'built-in', external: 'external', plugin: 'plugin' };
const STATE_TEXT = { pending: 'waiting for the daemon', ready: 'ready', failed: 'failed', too_old: 'daemon too old' };

function PluginHost({ st, nowMs }) {
  return html`<li class="plugin-host" data-testid="plugin-host-status" data-host=${st.host} data-state=${st.state}>
    <div class="plugin-host-head"><b>${st.host}</b><span class="ph-state" data-state=${st.state}>${STATE_TEXT[st.state] ?? st.state}</span>
      ${st.installed?.length ? html`<span class="chip" title=${st.installed.join('\n')}>${st.installed.length} installed</span>` : null}
      ${st.present?.length ? html`<span class="chip" title=${st.present.join('\n')}>${st.present.length} present</span>` : null}
      ${st.dropped?.length ? html`<span class="chip" title="claude on this host does not know these flags">dropped ${st.dropped.join(' ')}</span>` : null}</div>
    ${st.error ? html`<p class="ph-error">${st.error}</p>` : null}
    ${(st.skipped ?? []).map((k) => {
      const left = retryLeft(st, k, nowMs);
      return html`<div class="ph-skip" key=${k.id}><code>${k.id}</code><span class="ph-reason">${bareReason(k.reason)}</span>
        ${left === null ? null : html`<span class="ph-retry" data-testid="retry-countdown">${left > 0 ? `retrying in ${left} s` : 'retrying now…'}</span>`}</div>`;
    })}
  </li>`;
}

function ExtRow({ e, plugin, onToggle, onReload, nowMs }) {
  const statuses = Object.values(model.value.plugins).filter((st) => st.name === e.name);
  const isPlugin = e.source === 'plugin';
  return html`<li class="ext-row" data-testid="extension-row" data-name=${e.name} data-source=${e.source} data-enabled=${e.enabled}>
    <div class="ext-head">
      <input type="checkbox" class="toggle" data-testid="extension-toggle" aria-label=${`${e.name} on`} checked=${e.enabled} disabled=${!!e.refused} onChange=${(ev) => onToggle(e, ev.target.checked)} />
      <b class="ext-name">${e.name}</b>
      <span class="chip" data-source=${e.source}>${SOURCE[e.source] ?? e.source}</span>
      ${e.profile ? html`<span class="chip">profile ${e.profile}</span>` : null}
      ${plugin?.version ? html`<span class="chip">v${plugin.version}</span>` : null}
      <span class="topbar-spacer"></span>
      ${isPlugin ? html`<button type="button" class="btn btn-sm" data-testid="plugin-setup-btn" disabled=${!plugin || plugin.manifest !== 'ready'} title=${plugin && plugin.manifest !== 'ready' ? `manifest ${plugin.manifest}` : 'Choose what it may install'} onClick=${async () => { await openPluginSetup(e.name); onReload(); }}>Set up</button>` : null}
    </div>
    ${e.description ? html`<p class="ext-desc">${e.description}</p>` : null}
    ${e.refused ? html`<p class="ext-refused" role="alert">Not loaded: ${e.refused}</p>` : null}
    ${plugin && plugin.manifest !== 'ready' ? html`<p class="ext-refused">Manifest ${plugin.manifest}${plugin.manifest_error ? `: ${plugin.manifest_error}` : ''}</p>` : null}
    ${plugin && plugin.manifest === 'ready' && !plugin.choices_saved ? html`<p class="ext-note">Choices not saved yet: set it up to let it install.</p>` : null}
    ${(e.lenses?.length || e.actions?.length) ? html`<div class="ext-tags">
      ${(e.lenses ?? []).map((l) => html`<span class="chip" key=${`l:${l.name}`} title=${`${l.kind} lens`}>lens ${l.name}</span>`)}
      ${(e.actions ?? []).map((a) => html`<span class="chip" key=${`a:${a.id}`} title=${a.input ? `asks: ${a.input}` : 'runs at once'}>action ${a.title}</span>`)}</div>` : null}
    ${isPlugin ? (statuses.length ? html`<ul class="plugin-hosts">${statuses.map((st) => html`<${PluginHost} key=${st.host} st=${st} nowMs=${nowMs} />`)}</ul>`
      : html`<p class="ext-note">No host has answered for this plugin yet.</p>`) : null}
  </li>`;
}

export function ExtensionsScreen() {
  const [state, setState] = useState({ loading: true, exts: [], plugins: [], error: null });
  const [nowMs, setNow] = useState(Date.now());
  const rev = reload.value;
  const [nonce, setNonce] = useState(0);

  const load = async () => {
    try {
      const [x, p, st] = await Promise.all([reqQuiet('extensions.list'), reqQuiet('plugins.list').catch(() => ({ plugins: [] })), reqQuiet('plugins.status').catch(() => ({ status: [] }))]);
      for (const s of st.status ?? []) {
        const { host, name } = s;
        model.value = applyEvent(model.value, { ev: 'plugin_status', ...s });
        stamp(`${host}/${name}`);
      }
      setState({ loading: false, exts: x.extensions ?? [], plugins: p.plugins ?? [], error: null });
    } catch (e) {
      setState({ loading: false, exts: [], plugins: [], error: e.message });
    }
  };
  useEffect(() => { load(); }, [rev, nonce]);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  // a retry came due: the daemon asks again; so do we, a moment later (once per skipped item)
  useEffect(() => {
    let due = false;
    for (const st of Object.values(model.value.plugins)) {
      for (const k of st.skipped ?? []) if (retryLeft(st, k, nowMs) === 0 && !reasked.has(k)) { reasked.add(k); due = true; }
    }
    if (due) setTimeout(load, 1500);
  }, [Math.floor(nowMs / 1000)]);

  async function toggle(e, on) {
    setState((s) => ({ ...s, exts: s.exts.map((x) => (x.name === e.name ? { ...x, enabled: on } : x)) }));
    try { await req('extensions.toggle', { name: e.name, on }); } catch { setNonce((n) => n + 1); }
  }
  const byName = new Map(state.plugins.map((p) => [p.name, p]));
  return html`<${Dialog} title="Extensions" testid="extensions" wide onClose=${() => { overlay.value = null; }}>
    <div class="ext-bar"><span class="muted">${state.exts.length} extension${state.exts.length === 1 ? '' : 's'}. Changes apply to new sessions.</span>
      <button class="btn btn-sm" data-testid="extensions-reload" onClick=${() => setNonce(nonce + 1)}>Refresh</button></div>
    ${state.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the extensions: ${state.error}</p>` : null}
    ${state.loading ? html`<p class="panel-empty">Reading the extensions…</p>` : null}
    <ul class="ext-list">
      ${state.exts.map((e) => html`<${ExtRow} key=${e.name} e=${e} plugin=${byName.get(e.name)} nowMs=${nowMs} onToggle=${toggle} onReload=${() => setNonce((n) => n + 1)} />`)}
    </ul>
  <//>`;
}
