// The extensions screen: built-ins, external extensions and the addons' rows, with toggles; an
// addon's row shows the details its addon gives (addons.rows) and opens its setup checklist.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { req, reqQuiet, onServerEvent } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog } from './modal.js';
import { openChecklist } from './checklist.js';

const reload = signal(0); // bumped by extensions_changed and addons.changed

export function installExtensions() {
  onServerEvent((ev) => {
    if (ev.ev === 'extensions_changed' || ev.ev === 'addons.changed') reload.value++;
  });
  register({ id: 'extensions', group: 'App', title: 'Extensions', run: () => { overlay.value = 'extensions'; } });
}

const SOURCE = { builtin: 'built-in', external: 'external', addon: 'addon' };

function ExtRow({ e, row, onToggle, onReload }) {
  return html`<li class="ext-row" data-testid="extension-row" data-name=${e.name} data-source=${row ? 'addon' : e.source} data-enabled=${e.enabled}>
    <div class="ext-head">
      <input type="checkbox" class="toggle" data-testid="extension-toggle" aria-label=${`${e.name} on`} checked=${e.enabled} disabled=${!!e.refused} onChange=${(ev) => onToggle(e, row, ev.target.checked)} />
      <b class="ext-name">${e.name}</b>
      <span class="chip" data-source=${row ? 'addon' : e.source}>${row ? row.addon : SOURCE[e.source] ?? e.source}</span>
      <span class="topbar-spacer"></span>
      ${row?.setup ? html`<button type="button" class="btn btn-sm" data-testid="plugin-setup-btn" title="Choose what it may install" onClick=${async () => { await openChecklist({ addon: row.addon, id: row.setup }); onReload(); }}>Set up</button>` : null}
    </div>
    ${e.description ? html`<p class="ext-desc">${e.description}</p>` : null}
    ${e.refused ? html`<p class="ext-refused" role="alert">Not loaded: ${e.refused}</p>` : null}
    ${(row?.details ?? []).length ? html`<ul class="ext-details">${row.details.map((d, i) => html`<li key=${i} data-testid="extension-detail">${d}</li>`)}</ul>` : null}
    ${(e.lenses?.length || e.actions?.length) ? html`<div class="ext-tags">
      ${(e.lenses ?? []).map((l) => html`<span class="chip" key=${`l:${l.name}`} title=${`${l.kind} lens`}>lens ${l.name}</span>`)}
      ${(e.actions ?? []).map((a) => html`<span class="chip" key=${`a:${a.id}`} title=${a.input ? `asks: ${a.input}` : 'runs at once'}>action ${a.title}</span>`)}</div>` : null}
  </li>`;
}

export function ExtensionsScreen() {
  const [state, setState] = useState({ loading: true, exts: [], rows: [], error: null });
  const rev = reload.value;
  const [nonce, setNonce] = useState(0);

  const load = async () => {
    try {
      const [x, r] = await Promise.all([reqQuiet('extensions.list'), reqQuiet('addons.rows').catch(() => ({ rows: [] }))]);
      setState({ loading: false, exts: x.extensions ?? [], rows: r.rows ?? [], error: null });
    } catch (e) {
      setState({ loading: false, exts: [], rows: [], error: e.message });
    }
  };
  useEffect(() => { load(); }, [rev, nonce]);

  async function toggle(e, row, on) {
    setState((s) => ({ ...s, exts: s.exts.map((x) => (x.name === e.name ? { ...x, enabled: on } : x)) }));
    try {
      if (row) await req('addons.row_action', { addon: row.addon, name: row.name, action: 'toggle', on });
      else await req('extensions.toggle', { name: e.name, on });
    } catch { setNonce((n) => n + 1); }
  }
  // Refresh: every addon with rows fetches their data again, then the lists are read again.
  async function refresh() {
    const firsts = new Map();
    for (const r of state.rows) if (!firsts.has(r.addon)) firsts.set(r.addon, r.name);
    await Promise.all([...firsts].map(([addon, name]) => reqQuiet('addons.row_action', { addon, name, action: 'refresh' }).catch(() => {})));
    setNonce((n) => n + 1);
  }
  const rowOf = new Map(state.rows.map((r) => [r.name, r]));
  return html`<${Dialog} title="Extensions" testid="extensions" wide onClose=${() => { overlay.value = null; }}>
    <div class="ext-bar"><span class="muted">${state.exts.length} extension${state.exts.length === 1 ? '' : 's'}. Changes apply to new sessions.</span>
      <button class="btn btn-sm" data-testid="extensions-reload" onClick=${refresh}>Refresh</button></div>
    ${state.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the extensions: ${state.error}</p>` : null}
    ${state.loading ? html`<p class="panel-empty">Reading the extensions…</p>` : null}
    <ul class="ext-list">
      ${state.exts.map((e) => html`<${ExtRow} key=${e.name} e=${e} row=${rowOf.get(e.name) ?? null} onToggle=${toggle} onReload=${() => setNonce((n) => n + 1)} />`)}
    </ul>
  <//>`;
}
