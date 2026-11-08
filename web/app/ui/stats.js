// The Stats tab: proxy usage as SVG charts and segment gauges, pool health and the session's
// subscription. The credential of the focused session also feeds the top bar.
import { html } from 'htm/preact';
import { signal, computed, effect } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { model, conn, focused, req, reqQuiet } from '../store.js';
import { defineAction } from '../palette.js';
import { linearScale, niceTicks, barLayout, formatTokens } from '../charts.js';
import { level } from '../gauge.js';
import { SegBar } from './strip.js';
import { registerDrawerTab, toggleDrawer } from './drawer.js';
import { confirm } from './modal.js';
import { toast } from './toasts.js';

// ---- the subscription a session is on (proxy.credential), refreshed every 60 s while it matters ----
export const credentials = signal({}); // sid -> credential | null

export async function refreshCredential(sid) {
  try {
    const r = await reqQuiet('proxy.credential', { sid });
    credentials.value = { ...credentials.value, [sid]: r.credential ?? null };
  } catch { /* no proxy, no conversation yet or offline: keep what we had */ }
}

const credKey = computed(() => {
  const s = model.value.sessions[focused.value];
  return conn.value === 'open' && s && s.kind === 'claude' && s.proxy && s.claude_session_id ? s.id : null;
});
effect(() => {
  const sid = credKey.value;
  if (!sid) return undefined;
  refreshCredential(sid);
  const t = setInterval(() => refreshCredential(sid), 60000);
  return () => clearInterval(t);
});

// ---- small pieces ----
export function SegGauge({ pct, n = 16, testid, label }) {
  if (pct == null || !Number.isFinite(pct)) return html`<span class="gauge gauge-wide muted" data-testid=${testid}>${label ? html`<span class="gauge-label">${label}</span>` : null}–</span>`;
  const f = Math.min(1, Math.max(0, pct / 100));
  return html`<span class="gauge gauge-wide" data-testid=${testid} data-level=${level(f)} title=${`${label ?? ''} ${Math.round(pct)}%`.trim()}>
    ${label ? html`<span class="gauge-label">${label}</span>` : null}<${SegBar} f=${f} n=${n} /><span class="gauge-pct">${Math.round(pct)}%</span></span>`;
}

// The focused session's 5h / 7d usage for the top bar.
export function CredentialBar() {
  const sid = focused.value;
  const s = sid ? model.value.sessions[sid] : null;
  const c = sid ? credentials.value[sid] : null;
  if (!s || !s.proxy || !c) return null;
  return html`<span class="topbar-cred" data-testid="topbar-credential" title=${`${c.name ?? 'subscription'}${c.plan ? ` (${c.plan})` : ''} on ${s.proxy}`}>
    <${SegGauge} pct=${c.five_hour_pct} n=${6} label="5h" testid="topbar-gauge-5h" /><${SegGauge} pct=${c.seven_day_pct} n=${6} label="7d" testid="topbar-gauge-7d" /></span>`;
}

const W = 480;
const H = 224;
const M = { l: 46, r: 10, t: 18, b: 40 };

const shortModel = (m) => String(m).replace(/^claude-/, '');

// Output tokens per model as bars: Graphite with a Slate hairline and a 2px Phosphor top. Pure SVG.
export function BarChart({ rows }) {
  const pw = W - M.l - M.r;
  const ph = H - M.t - M.b;
  const values = rows.map((r) => r.output_tokens);
  const ticks = niceTicks(Math.max(0, ...values), 4);
  const top = ticks[ticks.length - 1];
  const y = linearScale([0, top], [ph, 0]);
  const bars = barLayout(values, pw, ph, 14);
  return html`<svg class="chart" data-testid="stats-chart" viewBox=${`0 0 ${W} ${H}`} role="img" aria-label="Output tokens per model" shape-rendering="crispEdges">
    <g transform=${`translate(${M.l} ${M.t})`}>
      ${ticks.map((t) => html`<g key=${t}><line class="chart-grid" x1="0" x2=${pw} y1=${y(t)} y2=${y(t)} /><text class="chart-tick" x="-8" y=${y(t) + 4} text-anchor="end">${formatTokens(t)}</text></g>`)}
      ${bars.map((b, i) => {
        const h = ph - y(values[i]);
        return html`<g key=${rows[i].model} class="chart-bar"><title>${`${rows[i].model}: ${values[i].toLocaleString()} output tokens, ${rows[i].requests.toLocaleString()} requests`}</title>
          <rect x=${Math.round(b.x) + 0.5} y=${Math.round(ph - h) + 0.5} width=${Math.max(0, Math.round(b.w) - 1)} height=${Math.max(0, Math.round(h) - 0.5)} />
          ${values[i] > 0 ? html`<rect class="chart-cap" x=${Math.round(b.x)} y=${Math.round(ph - h)} width=${Math.round(b.w)} height="2" />` : null}
          <text class="chart-value" x=${b.x + b.w / 2} y=${ph - h - 5} text-anchor="middle">${formatTokens(values[i])}</text>
          <text class="chart-label" x=${b.x + b.w / 2} y=${ph + 16} text-anchor="middle">${shortModel(rows[i].model)}</text>
        </g>`;
      })}
      <line class="chart-axis" x1="0" x2=${pw} y1=${ph} y2=${ph} />
    </g>
  </svg>`;
}

const WINDOWS = ['24h', '7d', '30d'];
const count = (n) => (Number.isFinite(n) ? n.toLocaleString() : '–');

function Tile({ label, value, tone = '', title }) {
  return html`<div class="tile" data-tone=${tone} title=${title}><span class="tile-v">${value}</span><span class="tile-l">${label}</span></div>`;
}

function until(sec) {
  if (!sec) return '';
  const d = new Date(sec * 1000);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleString(undefined, { dateStyle: 'short', timeStyle: 'short' });
}

function Subscription({ s }) {
  const c = credentials.value[s.id];
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!s.proxy || !s.claude_session_id) return undefined;
    refreshCredential(s.id);
    const t = setInterval(() => refreshCredential(s.id), 60000);
    return () => clearInterval(t);
  }, [s.id, s.proxy, s.claude_session_id]);
  if (!s.proxy) return html`<section class="stats-sec"><h3>Subscription</h3><p class="panel-note">This session does not use a proxy profile.</p></section>`;
  if (!s.claude_session_id) return html`<section class="stats-sec"><h3>Subscription</h3><p class="panel-note">The conversation has not started yet.</p></section>`;
  async function move() {
    const ok = await confirm({ title: 'Move to the next subscription?', body: `${s.label} keeps its conversation but continues on the next subscription of the ${s.proxy} pool.`, yes: 'Move', no: 'Keep' });
    if (!ok) return;
    setBusy(true);
    try {
      const r = await req('proxy.switch', { sid: s.id });
      toast({ text: `Subscription: ${r.from ?? 'none'} → ${r.to ?? 'none'}`, level: 'info' });
      await refreshCredential(s.id);
    } catch { /* req toasted */ } finally { setBusy(false); }
  }
  return html`<section class="stats-sec" data-testid="stats-credential">
    <h3>Subscription <span class="count">${s.proxy}</span></h3>
    ${c === undefined ? html`<p class="panel-note">Asking the proxy…</p>`
      : c === null ? html`<p class="panel-note">The proxy does not know this conversation yet.</p>`
      : html`<div class="cred-card">
        <div class="cred-name"><b>${c.name ?? 'unnamed'}</b> ${c.plan ? html`<span class="chip">${c.plan}</span>` : null} <span class="muted">${c.provider}</span></div>
        <div class="cred-gauges"><${SegGauge} pct=${c.five_hour_pct} n=${20} label="5h" testid="credential-gauge-5h" /><${SegGauge} pct=${c.seven_day_pct} n=${20} label="7d" testid="credential-gauge-7d" /></div>
        ${c.switched_secs_ago != null ? html`<p class="muted cred-note">switched ${formatSecs(c.switched_secs_ago)} ago</p>` : null}
      </div>`}
    <button class="btn btn-sm" data-testid="switch-btn" disabled=${busy} onClick=${move}>Move to next subscription</button>
  </section>`;
}

function formatSecs(n) {
  if (n < 90) return `${Math.round(n)} s`;
  if (n < 5400) return `${Math.round(n / 60)} min`;
  return `${Math.round(n / 3600)} h`;
}

export function StatsPanel({ sid }) {
  const s = model.value.sessions[sid];
  const names = model.value.profiles.names;
  const def = s?.proxy ?? model.value.profiles.default ?? names[0] ?? null;
  const [profile, setProfile] = useState(def);
  const [win, setWin] = useState('24h');
  const [data, setData] = useState({ loading: true, d: null, error: null });
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    if (!profile) { setData({ loading: false, d: null, error: null }); return undefined; }
    let dead = false;
    setData((p) => ({ ...p, loading: true, error: null }));
    reqQuiet('proxy.stats', { profile, window: win })
      .then((d) => { if (!dead) setData({ loading: false, d, error: null }); })
      .catch((e) => { if (!dead) setData({ loading: false, d: null, error: e.message }); });
    return () => { dead = true; };
  }, [profile, win, nonce]);
  if (!s) return null;
  const d = data.d;
  const t = d?.totals;
  const lim = d?.limit;
  const rows = (d?.by_model ?? []).slice().sort((a, b) => b.output_tokens - a.output_tokens);
  return html`<div class="stats" data-testid="stats">
    <div class="panel-bar">
      ${names.length ? html`<select class="select select-sm" data-testid="stats-profile" aria-label="Proxy profile" value=${profile ?? ''} onChange=${(e) => setProfile(e.target.value)}>
        ${names.map((n) => html`<option value=${n} selected=${n === profile}>${n}</option>`)}</select>` : null}
      <div class="seg" role="tablist" aria-label="Window">
        ${WINDOWS.map((w) => html`<button class="seg-btn" role="tab" key=${w} data-testid=${`stats-window-${w}`} aria-selected=${win === w} onClick=${() => setWin(w)}>${w}</button>`)}
      </div>
      <span class="topbar-spacer"></span>
      <button class="icon-btn" title="Reload" aria-label="Reload the stats" onClick=${() => setNonce(nonce + 1)}>↻</button>
    </div>
    <div class="stats-scroll">
      ${!profile ? html`<p class="panel-empty">No proxy profile is configured. Run <code>claudio proxy login</code>, then reload.</p>`
        : data.error ? html`<p class="panel-empty panel-error" role="alert">Could not read the stats: ${data.error}</p>`
        : !d ? html`<p class="panel-empty">Asking the proxy…</p>`
        : html`
        <p class="stats-who muted">${d.user_name ? `${d.user_name} · ` : ''}${d.profile} · last ${d.window}</p>
        <div class="tiles">
          <${Tile} label="requests" value=${count(t.requests)} />
          <${Tile} label="errors" value=${count(t.errors)} tone=${t.errors > 0 ? 'bad' : ''} />
          <${Tile} label="input" value=${formatTokens(t.input_tokens)} title=${count(t.input_tokens)} />
          <${Tile} label="output" value=${formatTokens(t.output_tokens)} title=${count(t.output_tokens)} />
          <${Tile} label="cache read" value=${formatTokens(t.cache_read)} title=${count(t.cache_read)} />
          <${Tile} label="cache write" value=${formatTokens(t.cache_creation)} title=${count(t.cache_creation)} />
        </div>
        <section class="stats-sec"><h3>Output tokens by model</h3>
          ${rows.length ? html`<${BarChart} rows=${rows} />` : html`<p class="panel-note">No requests in this window.</p>`}</section>
        ${lim ? html`<section class="stats-sec" data-testid="stats-limit"><h3>Limit <span class="count">${Math.round(lim.window_seconds / 3600)} h window</span></h3>
          <div class="limit-row"><${SegGauge} pct=${lim.used_pct} n=${24} testid="limit-gauge" />
            <span class="muted">${formatTokens(lim.used_output_tokens)} of ${formatTokens(lim.output_tokens)} output tokens</span></div>
          ${lim.blocked ? html`<p class="panel-note panel-error" role="alert">Blocked${lim.blocked_until ? ` until ${until(lim.blocked_until)}` : ''}.</p>` : null}</section>` : null}
        ${rows.length ? html`<section class="stats-sec"><h3>By model</h3><table class="stats-table">
          <thead><tr><th>model</th><th>req</th><th>err</th><th>in</th><th>out</th><th>cache r</th><th>cache w</th></tr></thead>
          <tbody>${rows.map((r) => html`<tr key=${r.model}><td>${shortModel(r.model)}</td><td>${count(r.requests)}</td><td>${count(r.errors)}</td><td>${formatTokens(r.input_tokens)}</td><td>${formatTokens(r.output_tokens)}</td><td>${formatTokens(r.cache_read)}</td><td>${formatTokens(r.cache_creation)}</td></tr>`)}</tbody></table></section>` : null}
        ${d.pool?.length ? html`<section class="stats-sec" data-testid="stats-pool"><h3>Pool health <span class="count">${d.pool.length}</span></h3>
          <ul class="pool">${d.pool.map((p) => html`<li key=${p.name} data-ok=${p.status === 'ok'}><span class="pool-dot" aria-hidden="true"></span><b>${p.name}</b><span class="muted">${p.status}</span></li>`)}</ul></section>` : null}`}
      <${Subscription} s=${s} />
    </div>
  </div>`;
}

registerDrawerTab('stats', { title: 'Stats', order: 40, Component: StatsPanel });
defineAction('stats', (sid) => toggleDrawer('stats', typeof sid === 'string' ? sid : undefined));
