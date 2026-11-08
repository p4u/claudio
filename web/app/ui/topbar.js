import { html } from 'htm/preact';
import { signal } from '@preact/signals';
import { useLayoutEffect, useRef } from 'preact/hooks';
import { model, conn, mode, serverInfo } from '../store.js';
import { overlay } from '../palette.js';
import { openHostPanel, MiniBar } from './hosts.js';
import { CredentialBar } from './stats.js';
import { updateOffered } from './claude.js';

const CONN = { open: 'live', connecting: 'connecting', closed: 'offline' };

// A host as a chip: connection dot, name, a glyph while it reconnects, an update badge and (in the
// top bar) cpu and memory mini bars. A click opens the host panel.
export function HostChip({ host, bars = false }) {
  const state = host.connected ? 'up' : host.connecting ? 'connecting' : 'down';
  const st = bars && host.connected ? model.value.stats[host.name] : null;
  const title = host.error ? `${host.name}: ${host.error}` : `${host.name}${host.version ? ` · claudio ${host.version}` : ''}${host.outdated ? ' · update pending' : ''}${host.connecting ? ' · connecting' : ''}`;
  const offer = model.value.claude[host.name];
  const upd = updateOffered(offer) ? offer : null;
  const open = (e) => { e.preventDefault(); e.stopPropagation(); openHostPanel(host.name); };
  return html`<button type="button" class="chip chip-btn" data-testid="host-chip" data-host=${host.name} data-conn=${state} title=${title} aria-label=${`Host ${host.name}, ${host.connected ? 'connected' : host.connecting ? 'connecting' : 'offline'}${upd ? ', claude update available' : ''}`} onClick=${open}>
    <span class="chip-dot"></span>${host.name}${host.connecting ? html`<span class="chip-glyph" aria-hidden="true" title="reconnecting">⇄</span>` : null}${host.outdated ? html`<span class="chip-badge" title="update pending">↑</span>` : null}${upd ? html`<span class="chip-badge chip-update" data-testid="claude-update-chip" title=${`claude ${upd.have ?? '?'} → ${upd.want ?? '?'} is available. Click to update.`}>↑</span>` : null}
    ${st ? html`<span class="chip-bars"><${MiniBar} value=${st.cpu} label="cpu" /><${MiniBar} value=${st.mem_total ? (100 * st.mem_used) / st.mem_total : 0} label="mem" /></span>` : null}
  </button>`;
}

const hostsOpen = signal(false);

const hostState = (h) => (h.connected ? 'up' : h.connecting ? 'connecting' : 'down');

// One line for the button: how many hosts are up and a glyph when one needs a look.
export function hostsSummary(hosts) {
  const up = hosts.filter((h) => h.connected).length;
  const glyph = hosts.some((h) => h.error && !h.connected) ? '!' : hosts.some((h) => h.connecting) ? '⇄' : hosts.some((h) => h.outdated) ? '↑' : '';
  return { up, total: hosts.length, glyph };
}

// A row of the hosts menu: state glyph, name, cpu and memory bars, claude version, `↑` when an
// update is offered. A click opens the host panel.
function HostRow({ host, onPick }) {
  const state = hostState(host);
  const st = host.connected ? model.value.stats[host.name] : null;
  const cl = model.value.claude[host.name];
  const upd = updateOffered(cl) ? cl : null;
  const ver = cl?.have ?? cl?.version ?? null;
  const g = host.connecting ? '⇄' : host.connected ? '●' : '○';
  const title = host.error ? `${host.name}: ${host.error}` : `${host.name}${host.version ? ` · claudio ${host.version}` : ''}${host.outdated ? ' · update pending' : ''}${host.connecting ? ' · connecting' : ''}`;
  return html`<button type="button" role="menuitem" class="hosts-row" data-testid="host-chip" data-host=${host.name} data-conn=${state} title=${title}
    aria-label=${`Host ${host.name}, ${host.connected ? 'connected' : host.connecting ? 'connecting' : 'offline'}${upd ? ', claude update available' : ''}`} onClick=${() => onPick(host.name)}>
    <span class="hosts-glyph" aria-hidden="true">${g}</span><span class="hosts-name" title=${host.name}>${host.name}</span>
    <span class="chip-bars">${st ? html`<${MiniBar} value=${st.cpu} label="cpu" /><${MiniBar} value=${st.mem_total ? (100 * st.mem_used) / st.mem_total : 0} label="mem" />` : null}</span>
    <span class="hosts-ver">${ver ?? ''}</span>
    ${host.outdated ? html`<span class="chip-badge" title="update pending">↑</span>` : null}${upd ? html`<span class="chip-badge chip-update" data-testid="claude-update-chip" title=${`claude ${upd.have ?? '?'} → ${upd.want ?? '?'} is available.`}>↑</span>` : null}
  </button>`;
}

function HostsMenu({ hosts }) {
  const wrap = useRef(null);
  const sum = hostsSummary(hosts);
  const items = () => [...wrap.current.querySelectorAll('[role="menuitem"]')];
  const close = (refocus) => { hostsOpen.value = false; if (refocus) wrap.current?.querySelector('[data-testid="hosts-btn"]')?.focus(); };
  const open = hostsOpen.value;
  useLayoutEffect(() => {
    if (!open) return undefined;
    items()[0]?.focus();
    const away = (e) => { if (wrap.current && !wrap.current.contains(e.target)) close(false); };
    document.addEventListener('pointerdown', away, true);
    return () => document.removeEventListener('pointerdown', away, true);
  }, [open]);
  const pick = (name) => { close(false); openHostPanel(name); };
  const onKey = (e) => {
    const list = items();
    const i = list.indexOf(document.activeElement);
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(true); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); list[(i + 1) % list.length]?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); list[(i - 1 + list.length) % list.length]?.focus(); }
    else if (e.key === 'Home') { e.preventDefault(); list[0]?.focus(); }
    else if (e.key === 'End') { e.preventDefault(); list[list.length - 1]?.focus(); }
    else if (e.key === 'Tab') close(false);
  };
  return html`<div class="hosts-wrap" ref=${wrap}>
    <button type="button" class="nav-btn hosts-btn" data-testid="hosts-btn" aria-haspopup="menu" aria-expanded=${open} title="Hosts"
      onClick=${() => { hostsOpen.value = !open; }}
      onKeyDown=${(e) => { if (e.key === 'ArrowDown' && !open) { e.preventDefault(); hostsOpen.value = true; } }}>
      Hosts <span class="hosts-sum" data-testid="hosts-summary">${sum.up}/${sum.total}</span>${sum.glyph ? html`<span class="chip-glyph" aria-hidden="true">${sum.glyph}</span>` : null}
    </button>
    ${open ? html`<div class="hosts-menu popover" data-testid="hosts-menu" role="menu" aria-label="Hosts" onKeyDown=${onKey}>
      ${hosts.map((h) => html`<${HostRow} key=${h.name} host=${h} onPick=${pick} />`)}
      <button type="button" role="menuitem" class="hosts-row hosts-add" data-testid="host-add-chip" onClick=${() => pick('')}>Add host…</button>
    </div>` : null}
  </div>`;
}

export function attentionCount(m) {
  return Object.values(m.sessions).filter((s) => s.state === 'needs_approval' || s.state === 'needs_input').length;
}

export function Topbar() {
  const m = model.value;
  const hosts = Object.values(m.hosts).sort((a, b) => (a.name === 'local' ? -1 : b.name === 'local' ? 1 : a.name.localeCompare(b.name)));
  const attn = attentionCount(m);
  return html`<header class="topbar">
    <div class="logo">claudi<span class="logo-accent">o</span></div>
    <${HostsMenu} hosts=${hosts} />
    <div class="topbar-spacer"></div>
    <${CredentialBar} />
    ${attn > 0 ? html`<span class="badge badge-attn" title="sessions waiting for you">${attn} waiting</span>` : null}
    <span class="badge conn" data-conn=${conn.value} title=${serverInfo.value.version ? `claudio ${serverInfo.value.version}` : ''}>${CONN[conn.value]}</span>
    <span class="badge" data-testid="mode-badge" data-mode=${mode.value} title="Ctrl+Shift+Space leaves the terminal">${mode.value === 'terminal' ? 'TERM' : 'APP'}</span>
    <div class="topbar-actions">
      <button class="nav-btn" onClick=${() => { overlay.value = 'palette'; }} title="Command palette">Palette <kbd>K</kbd></button>
      <button class="icon-btn" onClick=${() => { overlay.value = 'help'; }} title="Keys" aria-label="Keys">?</button>
      <button class="nav-btn" onClick=${() => { overlay.value = 'settings'; }} title="Settings">Settings</button>
    </div>
  </header>`;
}
