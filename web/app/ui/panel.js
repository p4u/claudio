// An addon's panel (docs/web-protocol.md §4 "Addons", §7): page tabs, window buttons, blocks drawn
// as text nodes, tables and CSS-width bars, and the panel's actions. Also the addons' declarations
// (`addons.list`), which the wizard, the palette and the session actions read.
import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { model, focused, reqQuiet, onServerEvent } from '../store.js';
import { defineAction } from '../palette.js';
import { barRows, gaugeOf, short, tone, panelFor } from '../addon-view.js';
import { Dialog, confirm } from './modal.js';
import { toast } from './toasts.js';
import { ago } from './glyph.js';

// ---- the addons' declarations, read again on every snapshot and addons.changed ----
export const addonDecls = signal([]);
// The actions each panel offered when it was last read: `${addon}/${panel}` -> [PanelAction].
export const panelActions = signal({});

export async function loadAddonDecls() {
  try {
    const r = await reqQuiet('addons.list');
    addonDecls.value = r.addons ?? [];
  } catch { /* an older server or offline: keep what we had */ }
  return addonDecls.value;
}

// Alt+s (`stats`): the panel whose TUI key is alt+s, else the first, about `sid` or the focused
// session.
export function installAddons() {
  onServerEvent((ev) => {
    if (ev.ev === 'snapshot' || ev.ev === 'addons.changed') loadAddonDecls();
  });
  defineAction('stats', (sid) => {
    const p = panelFor(addonDecls.value, 'alt+s');
    if (p) openPanel({ addon: p.addon, panel: p.panel.id, sid: typeof sid === 'string' ? sid : focused.value });
  });
}

// ---- the open panel ----
export const panelOpen = signal(null); // {addon, panel, sid}
export const closePanel = () => { panelOpen.value = null; };

// openPanel({addon, panel, sid}): `sid` the session it is about (its launch picks the profile).
export function openPanel({ addon, panel, sid = null }) {
  panelOpen.value = { addon, panel, sid: sid && model.value.sessions[sid] ? sid : null };
}

export function declOf(addon, panel) {
  return addonDecls.value.find((d) => d.name === addon)?.panels?.find((p) => p.id === panel) ?? null;
}

// Run a panel action: its confirm first, then the addon's Notice as a toast.
export async function runPanelAction({ addon, panel, sid }, a) {
  if (a.confirm && !(await confirm({ title: a.label, body: a.confirm, yes: a.label, no: 'Cancel' }))) return false;
  try {
    const n = await reqQuiet('addons.action', { addon, panel, action: a.id, ...(sid ? { sid } : {}) });
    toast({ text: n?.text ?? `${a.label}: done`, level: ['info', 'warn', 'error'].includes(n?.level) ? n.level : 'info' });
    return true;
  } catch (e) {
    toast({ text: `${a.label}: ${e.message}`, level: 'error' });
    return false;
  }
}

function Block({ b }) {
  switch (b.kind) {
    case 'text':
      return html`<p class="pnl-text" data-testid="panel-text" data-tone=${b.tone ? tone(b.tone) : null}>${b.text}</p>`;
    case 'key_values':
      return html`<dl class="pnl-kv" data-testid="panel-kv">${(b.items ?? []).map(([k, v], i) => html`<div class="kv" key=${i}><dt>${k}</dt><dd>${v}</dd></div>`)}</dl>`;
    case 'table':
      return html`<div class="pnl-table-wrap"><table class="pnl-table" data-testid="panel-table">
        <thead><tr>${(b.header ?? []).map((h, i) => html`<th key=${i}>${h}</th>`)}</tr></thead>
        <tbody>${(b.rows ?? []).map((r, i) => html`<tr key=${i}>${r.map((c, j) => html`<td key=${j}>${c}</td>`)}</tr>`)}</tbody>
      </table></div>`;
    case 'gauge': {
      const g = gaugeOf(b);
      return html`<div class="pnl-gauge" data-testid="panel-gauge" data-level=${g.level}>
        <div class="pnl-label"><span>${b.label}</span><span class="pnl-num">${short(g.used)} / ${short(g.limit)} · ${Math.round(g.pct)}%</span></div>
        <div class="pnl-track" role="meter" aria-label=${b.label} aria-valuemin="0" aria-valuemax=${g.limit} aria-valuenow=${g.used}><div class="pnl-fill" style=${{ width: `${g.pct}%` }}></div></div>
        ${b.resets ? html`<p class="pnl-note">${b.resets}</p>` : null}
      </div>`;
    }
    case 'bars':
      return html`<div class="pnl-bars" data-testid="panel-bars">
        <div class="pnl-label"><span>${b.label}</span></div>
        ${barRows(b.series).map((r, i) => html`<div class="pnl-bar" key=${i}>
          <span class="pnl-bar-l">${r.label}</span>
          <span class="pnl-track"><span class="pnl-fill" style=${{ width: `${r.pct}%` }}></span></span>
          <span class="pnl-num">${short(r.value)}</span>
        </div>`)}
      </div>`;
    default:
      return null;
  }
}

function PanelView({ p }) {
  const decl = declOf(p.addon, p.panel);
  const windows = decl?.windows ?? [];
  const [win, setWin] = useState(windows[0] ?? null);
  const [page, setPage] = useState(0);
  const [data, setData] = useState({ loading: true, d: null, error: null });
  const [nonce, setNonce] = useState(0);
  const [busy, setBusy] = useState(false);
  useEffect(() => onServerEvent((ev) => { if (ev.ev === 'addons.changed' && ev.addon === p.addon) setNonce((n) => n + 1); }), [p.addon]);
  useEffect(() => {
    let dead = false;
    setData((x) => ({ ...x, loading: true }));
    const args = { addon: p.addon, panel: p.panel, page, ...(win ? { window: win } : {}), ...(p.sid ? { sid: p.sid } : {}) };
    reqQuiet('addons.panel', args)
      .then((d) => {
        if (dead) return;
        setData({ loading: false, d, error: null });
        panelActions.value = { ...panelActions.value, [`${p.addon}/${p.panel}`]: d.actions ?? [] };
      })
      .catch((e) => { if (!dead) setData((x) => ({ loading: false, d: x.d, error: e.message })); });
    return () => { dead = true; };
  }, [p.addon, p.panel, p.sid, win, page, nonce]);
  const d = data.d;
  const pages = d?.pages ?? [];
  const at = Math.min(page, Math.max(0, pages.length - 1));
  const cur = pages[at];
  const s = p.sid ? model.value.sessions[p.sid] : null;
  async function act(a) {
    setBusy(true);
    if (await runPanelAction(p, a)) setNonce((n) => n + 1);
    setBusy(false);
  }
  return html`<${Dialog} title=${d?.title || decl?.title || p.panel} testid="panel" wide extra="panel-dialog" onClose=${closePanel}>
    <div class="pnl-head">
      ${pages.length > 1 ? html`<div class="seg pnl-pages" role="tablist" aria-label="Pages">
        ${pages.map((pg, i) => html`<button type="button" class="seg-btn" role="tab" key=${i} data-testid=${`panel-page-${i}`} aria-selected=${i === at} onClick=${() => setPage(i)}>${pg.title}</button>`)}
      </div>` : null}
      <span class="topbar-spacer"></span>
      ${windows.length > 1 ? html`<div class="seg" role="radiogroup" aria-label="Window">
        ${windows.map((w) => html`<button type="button" class="seg-btn" role="radio" key=${w} data-testid=${`panel-window-${w}`} aria-checked=${w === win} onClick=${() => setWin(w)}>${w}</button>`)}
      </div>` : null}
      <button type="button" class="icon-btn" data-testid="panel-reload" title="Read it again" aria-label="Read it again" onClick=${() => setNonce(nonce + 1)}>↻</button>
    </div>
    ${s ? html`<p class="pnl-about muted">About ${s.label}${s.host !== 'local' ? ` @${s.host}` : ''}</p>` : null}
    <div class="pnl-body" data-testid="panel-body" data-loading=${data.loading}>
      ${data.error && !d ? html`<p class="panel-empty panel-error" role="alert">Could not read it: ${data.error}</p>`
        : !d ? html`<p class="panel-empty">Reading…</p>`
        : (cur?.blocks ?? []).map((b, i) => html`<${Block} key=${`${at}:${i}`} b=${b} />`)}
    </div>
    <div class="dialog-actions pnl-foot">
      ${d?.updated ? html`<span class="pnl-updated muted" data-testid="panel-updated">updated ${ago(d.updated)} ago</span>` : null}
      <span class="topbar-spacer"></span>
      ${(d?.actions ?? []).map((a) => html`<button type="button" class="btn" key=${a.id} data-testid="panel-action" data-action=${a.id} disabled=${busy} title=${a.key ? `${a.label} (${a.key})` : a.label} onClick=${() => act(a)}>${a.label}</button>`)}
      <button type="button" class="btn" data-testid="panel-close" onClick=${closePanel}>Close</button>
    </div>
  <//>`;
}

export function PanelHost() {
  const p = panelOpen.value;
  return p ? html`<${PanelView} key=${`${p.addon}/${p.panel}/${p.sid ?? ''}`} p=${p} />` : null;
}
