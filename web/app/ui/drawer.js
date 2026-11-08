// The drawer: a column beside the stage with one tab per registered panel. Task 9 registers the
// reader; later tasks call registerDrawerTab for changes, history, stats and session.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, focused, selected } from '../store.js';
import { drawer, openDrawer } from './session-actions.js';

export const drawerTabs = signal([]); // [{ name, title, order, Component }]

// registerDrawerTab(name, { title, order?, Component }) -> unregister. Component gets { sid }.
export function registerDrawerTab(name, { title, order = 100, Component }) {
  const tab = { name, title, order, Component };
  drawerTabs.value = [...drawerTabs.value.filter((t) => t.name !== name), tab].sort((a, b) => a.order - b.order);
  return () => { drawerTabs.value = drawerTabs.value.filter((t) => t !== tab); };
}

export function closeDrawer() {
  drawer.value = null;
}

// `r`-style toggle: close when that tab is already showing for the session, else open it.
export function toggleDrawer(tab, sid = focused.value ?? selected.value) {
  const d = drawer.value;
  if (d && d.tab === tab && (!sid || d.sid === sid)) closeDrawer();
  else openDrawer(tab, sid);
}

export function Drawer() {
  const d = drawer.value;
  const s = d ? model.value.sessions[d.sid] : null;
  if (!d || !s) return null;
  const tabs = drawerTabs.value;
  const cur = tabs.find((t) => t.name === d.tab) ?? tabs[0];
  return html`<aside class="drawer" data-testid="drawer" data-tab=${cur?.name ?? ''} aria-label="Session details">
    <header class="drawer-head">
      <div class="drawer-tabs" role="tablist">
        ${tabs.map((t) => html`<button key=${t.name} class="drawer-tab" role="tab" data-testid=${`drawer-tab-${t.name}`} aria-selected=${cur?.name === t.name}
          onClick=${() => openDrawer(t.name, d.sid)}>${t.title}</button>`)}
      </div>
      <span class="drawer-title" title=${s.label}>${s.label}</span>
      <button class="icon-btn" aria-label="Close the drawer" title="Close" onClick=${closeDrawer}>×</button>
    </header>
    <div class="drawer-body">${cur ? html`<${cur.Component} key=${`${cur.name}:${d.sid}`} sid=${d.sid} />` : null}</div>
  </aside>`;
}
