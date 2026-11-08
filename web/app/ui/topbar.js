import { html } from 'htm/preact';
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

export function attentionCount(m) {
  return Object.values(m.sessions).filter((s) => s.state === 'needs_approval' || s.state === 'needs_input').length;
}

export function Topbar() {
  const m = model.value;
  const hosts = Object.values(m.hosts).sort((a, b) => (a.name === 'local' ? -1 : b.name === 'local' ? 1 : a.name.localeCompare(b.name)));
  const attn = attentionCount(m);
  return html`<header class="topbar">
    <div class="logo"><span class="logo-mark" aria-hidden="true"></span>CLAUDIO</div>
    <div class="topbar-hosts">${hosts.map((h) => html`<${HostChip} key=${h.name} host=${h} bars />`)}
      <button type="button" class="chip chip-btn chip-add" data-testid="host-add-chip" title="Add a host" aria-label="Add a host" onClick=${() => openHostPanel('')}>+</button></div>
    <div class="topbar-spacer"></div>
    <${CredentialBar} />
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
