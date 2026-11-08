// A proxy plugin's setup screen: what the plugin may install and enable, one checkbox per manifest
// item. It opens before the first spawn on a profile that has an unsaved plugin (setBeforeSpawn)
// and from the extensions screen.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { req, reqQuiet } from '../store.js';
import { setBeforeSpawn } from '../spawn-hooks.js';
import { Dialog } from './modal.js';

export const setup = signal(null); // {name, finish}
export const setupOpen = () => setup.value !== null;
export const cancelPluginSetup = () => setup.value?.finish('cancelled');

// -> Promise<'saved' | 'skipped' | 'cancelled'>
// `starting`: the session that starts once this closes (the subtitle says so).
export function openPluginSetup(name, starting = null) {
  return new Promise((resolve) => {
    setup.value?.finish('cancelled');
    const entry = { name, starting };
    entry.finish = (v) => {
      if (setup.value === entry) setup.value = null;
      resolve(v);
    };
    setup.value = entry;
  });
}

// Group items by their section heading, keeping the manifest's order.
export function bySection(items) {
  const out = [];
  for (const it of items) {
    let g = out.find((x) => x.section === it.section);
    if (!g) { g = { section: it.section, items: [] }; out.push(g); }
    g.items.push(it);
  }
  return out;
}

function PluginSetup({ entry }) {
  const [state, setState] = useState({ loading: true, items: [], version: null, error: null });
  const [off, setOff] = useState(new Set()); // ids switched off
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let dead = false;
    reqQuiet('plugins.items', { name: entry.name })
      .then((r) => {
        if (dead) return;
        setState({ loading: false, items: r.items ?? [], version: r.version ?? null, error: null });
        setOff(new Set((r.items ?? []).filter((i) => !i.checked).map((i) => i.id)));
      })
      .catch((e) => { if (!dead) setState({ loading: false, items: [], version: null, error: e.message }); });
    return () => { dead = true; };
  }, [entry.name]);
  const toggle = (id) => setOff((cur) => { const n = new Set(cur); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  const all = state.items.length;
  const on = all - off.size;
  async function save() {
    setBusy(true);
    try {
      await req('plugins.save', { name: entry.name, disabled: [...off] });
      entry.finish('saved');
    } catch { setBusy(false); /* req toasted; stay open */ }
  }
  return html`<${Dialog} title=${`Set up ${entry.name}`} testid="plugin-setup" wide onClose=${() => entry.finish('cancelled')}>
    ${entry.starting ? html`<p class="setup-starting" data-testid="plugin-starting">Starting ${entry.starting} after this…</p>` : null}
    <p class="dialog-body">Choose what <b>${entry.name}</b>${state.version ? ` ${state.version}` : ''} may install and enable for your sessions. You can change this later in Extensions.</p>
    <div class="setup-bar">
      <span class="muted">${on} of ${all} on</span>
      <span class="setup-bulk">
        <button type="button" class="btn btn-sm" data-testid="plugin-all" onClick=${() => setOff(new Set())}>All</button>
        <button type="button" class="btn btn-sm" data-testid="plugin-none" onClick=${() => setOff(new Set(state.items.map((i) => i.id)))}>None</button>
      </span>
    </div>
    <div class="setup-items">
      ${state.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the plugin: ${state.error}</p>`
        : state.loading ? html`<p class="panel-empty">Reading the manifest…</p>` : null}
      ${bySection(state.items).map((g) => html`<section class="setup-sec" key=${g.section}>
        <h3>${g.section} <span class="count">${g.items.length}</span></h3>
        ${g.items.map((it) => html`<label class="setup-item" key=${it.id}>
          <input type="checkbox" class="toggle" data-testid="plugin-item" data-id=${it.id} checked=${!off.has(it.id)} onChange=${() => toggle(it.id)} />
          <span class="setup-title">${it.title}</span><code class="setup-id">${it.id}</code>
        </label>`)}
      </section>`)}
    </div>
    <div class="dialog-actions">
      <button type="button" class="btn" data-testid="plugin-skip" title="Start without this plugin this time" onClick=${() => entry.finish('skipped')}>Skip for now</button>
      <button type="button" class="btn btn-primary" data-testid="plugin-save" disabled=${busy || state.loading || !!state.error} onClick=${save}>Save</button>
    </div>
  <//>`;
}

export function PluginSetupHost() {
  const e = setup.value;
  return e ? html`<${PluginSetup} key=${e.name} entry=${e} />` : null;
}

// Before a claude session spawns on `profile`: every enabled plugin of it whose choices were never
// saved opens its setup in turn. Save continues, Skip leaves it out, Escape cancels the spawn.
export function installPluginGate() {
  setBeforeSpawn(async (profile, starting) => {
    if (!profile) return { skip: [] };
    let plugins;
    try { plugins = (await reqQuiet('plugins.list')).plugins ?? []; } catch { return { skip: [] }; }
    const skip = [];
    for (const p of plugins) {
      if (!p.enabled || p.profile !== profile || p.manifest !== 'ready' || p.choices_saved || p.refused) continue;
      const r = await openPluginSetup(p.name, starting);
      if (r === 'cancelled') return null;
      if (r === 'skipped') skip.push(p.name);
    }
    return { skip };
  });
}
