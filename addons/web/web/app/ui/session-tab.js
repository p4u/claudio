// The Session tab: every fact about one session, and its actions.
import { html } from 'htm/preact';
import { model } from '../store.js';
import { defineAction } from '../palette.js';
import { registerDrawerTab, toggleDrawer } from './drawer.js';
import { SessionActions } from './session-actions.js';
import { Glyph, ago, STATE_NAME } from './glyph.js';
import { gitText } from './strip.js';
import { Segments } from './segments.js';
import { labelFor } from '../launch.js';
import { formatTokens } from '../charts.js';

function Row({ k, v, mono = false }) {
  if (v === null || v === undefined || v === '') return null;
  return html`<div class="kv"><dt>${k}</dt><dd class=${mono ? 'mono' : ''}>${v}</dd></div>`;
}

export function SessionTab({ sid }) {
  const s = model.value.sessions[sid];
  if (!s) return null;
  const h = model.value.hosts[s.host];
  const shell = s.kind === 'shell';
  const g = s.git;
  return html`<div class="session-tab" data-testid="session-tab">
    <div class="panel-bar"><${Glyph} s=${s} /><b class="panel-title">${s.label}</b><span class="topbar-spacer"></span><${SessionActions} s=${s} /></div>
    <dl class="kv-list">
      <${Row} k="State" v=${`${STATE_NAME[s.state] ?? s.state}${s.state_since ? ` for ${ago(s.state_since)}` : ''}`} />
      <${Row} k="Host" v=${h?.hostname && h.hostname !== s.host ? `${s.host} (${h.hostname})` : s.host} />
      <${Row} k="Kind" v=${shell ? 'terminal' : 'claude'} />
      <${Row} k="Folder" v=${s.cwd} mono />
      <${Row} k="Session" v=${s.id} mono />
      <${Row} k="Conversation" v=${s.claude_session_id} mono />
      ${shell ? null : html`
        <${Row} k="Launch" v=${labelFor(s.launch)} />
        <${Row} k="Model" v=${s.model} />
        <${Row} k="Context" v=${s.context_tokens != null ? `${formatTokens(s.context_tokens)} tokens` : null} />
        <${Row} k="Output" v=${s.output_tokens != null ? `${formatTokens(s.output_tokens)} tokens` : null} />
        <${Row} k="Turns" v=${s.turns} />`}
      <${Row} k="Branch" v=${s.branch} />
      <${Row} k="Git" v=${g ? `${g.detached ? 'detached ' : ''}${g.head}${gitText(g) ? `  ${gitText(g)}` : ''}${g.upstream ? '' : '  (no upstream)'}` : null} />
      ${s.segments?.length ? html`<div class="kv"><dt>Addons</dt><dd><${Segments} s=${s} testid="session-segments" /></dd></div>` : null}
      <${Row} k="Process" v=${s.pid != null ? `pid ${s.pid}` : s.dormant ? 'dormant (no process)' : null} />
      <${Row} k="Started" v=${s.created_at ? `${ago(s.created_at)} ago` : null} />
      ${s.ephemeral ? html`<${Row} k="Ephemeral" v="yes" />` : null}
    </dl>
  </div>`;
}

registerDrawerTab('session', { title: 'Session', order: 50, Component: SessionTab });
defineAction('session', (sid) => toggleDrawer('session', typeof sid === 'string' ? sid : undefined));
