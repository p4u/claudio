import { html } from 'htm/preact';
import { model, conn, mode, serverInfo } from '../store.js';
import { overlay } from '../palette.js';

const CONN = { open: 'live', connecting: 'connecting', closed: 'offline' };

export function HostChip({ host }) {
  const state = host.connected ? 'up' : host.connecting ? 'connecting' : 'down';
  const title = host.error ? `${host.name}: ${host.error}` : `${host.name}${host.version ? ` · claudio ${host.version}` : ''}${host.outdated ? ' · update pending' : ''}`;
  return html`<span class="chip" data-testid="host-chip" data-host=${host.name} data-conn=${state} title=${title}>
    <span class="chip-dot"></span>${host.name}${host.outdated ? html`<span aria-label="update pending"> ↑</span>` : null}</span>`;
}

export function attentionCount(m) {
  return Object.values(m.sessions).filter((s) => s.state === 'needs_approval' || s.state === 'needs_input').length;
}

export function Topbar() {
  const m = model.value;
  const hosts = Object.values(m.hosts).sort((a, b) => (a.name === 'local' ? -1 : b.name === 'local' ? 1 : a.name.localeCompare(b.name)));
  const attn = attentionCount(m);
  return html`<header class="topbar">
    <div class="logo"><span class="logo-mark" aria-hidden="true"></span>CLAUDIO</div>
    <div class="topbar-hosts">${hosts.map((h) => html`<${HostChip} key=${h.name} host=${h} />`)}</div>
    <div class="topbar-spacer"></div>
    ${attn > 0 ? html`<span class="badge badge-attn" title="sessions waiting for you">${attn} waiting</span>` : null}
    <span class="conn" data-conn=${conn.value} title=${serverInfo.value.version ? `claudio ${serverInfo.value.version}` : ''}>${CONN[conn.value]}</span>
    <span class="badge" data-testid="mode-badge" data-mode=${mode.value} title="Ctrl+Shift+Space leaves the terminal">${mode.value === 'terminal' ? 'TERM' : 'APP'}</span>
    <div class="topbar-actions">
      <button class="btn btn-sm" onClick=${() => { overlay.value = 'palette'; }} title="Command palette">Palette <kbd>K</kbd></button>
      <button class="btn btn-sm" onClick=${() => { overlay.value = 'help'; }} title="Keys" aria-label="Keys">?</button>
      <button class="btn btn-sm" onClick=${() => { overlay.value = 'settings'; }} title="Settings">Settings</button>
    </div>
  </header>`;
}
