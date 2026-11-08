// The drawer: a column beside the stage with one tab per registered panel. Task 9 registers the
// reader; later tasks call registerDrawerTab for changes, history, stats and session.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { model, focused, selected } from '../store.js';
import { drawer, openDrawer } from './session-actions.js';
import { settings, updateSettings } from '../settings.js';
import { clampDrawer, DRAWER_MIN, DRAWER_MAX, DRAWER_DEFAULT } from '../layout.js';
import { viewportW } from './viewport.js';

export const drawerTabs = signal([]); // [{ name, title, order, Component }]

// registerDrawerTab(name, { title, order?, Component, disabled? }) -> unregister. Component gets { sid }.
// disabled(session) -> a reason (the tab is greyed out with it as its tooltip) or null.
export function registerDrawerTab(name, { title, order = 100, Component, disabled = null }) {
  const tab = { name, title, order, Component, disabled };
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

// The drag handle on the drawer's left edge. The width is the distance to the window's right edge.
function Resizer({ shown }) {
  const set = (w) => updateSettings({ drawerWidth: clampDrawer(w) });
  const down = (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    el.dataset.drag = 'true';
    const move = (m) => set(viewportW.value - m.clientX);
    const up = () => {
      delete el.dataset.drag;
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  };
  const key = (e) => {
    const cur = settings.value.drawerWidth;
    if (e.key === 'ArrowLeft') { e.preventDefault(); set(cur + 24); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); set(cur - 24); }
    else if (e.key === 'Home') { e.preventDefault(); set(DRAWER_DEFAULT); }
  };
  return html`<div class="drawer-resizer" data-testid="drawer-resizer" role="separator" aria-orientation="vertical" tabindex="0"
    aria-label="Resize the drawer" title="Drag to resize; double-click resets" aria-valuemin=${DRAWER_MIN} aria-valuemax=${DRAWER_MAX} aria-valuenow=${shown}
    onPointerDown=${down} onKeyDown=${key} onDblClick=${() => set(DRAWER_DEFAULT)}></div>`;
}

export function Drawer({ plan }) {
  const d = drawer.value;
  const s = d ? model.value.sessions[d.sid] : null;
  if (!d || !s) return null;
  const tabs = drawerTabs.value;
  const cur = tabs.find((t) => t.name === d.tab) ?? tabs[0];
  return html`<aside class="drawer" data-testid="drawer" data-tab=${cur?.name ?? ''} aria-label="Session details">
    <${Resizer} shown=${Math.round(plan?.drawerW ?? settings.value.drawerWidth)} />
    <header class="drawer-head">
      <div class="drawer-tabs" role="tablist">
        ${tabs.map((t) => {
          const why = t.disabled?.(s) ?? null;
          return html`<button key=${t.name} class="drawer-tab" role="tab" data-testid=${`drawer-tab-${t.name}`} aria-selected=${cur?.name === t.name}
            disabled=${!!why} title=${why ?? t.title} onClick=${() => openDrawer(t.name, d.sid)}>${t.title}</button>`;
        })}
      </div>
      <span class="drawer-title" title=${s.label}>${s.label}</span>
      <button class="icon-btn" aria-label="Close the drawer" title="Close" onClick=${closeDrawer}>×</button>
    </header>
    <div class="drawer-body">${cur ? html`<${cur.Component} key=${`${cur.name}:${d.sid}`} sid=${d.sid} />` : null}</div>
  </aside>`;
}
