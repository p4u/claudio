// Development mock of `claudio web` (docs/web-protocol.md, protocol v1).
//   node web/dev/mock-server.mjs     serves web/ on 127.0.0.1:${MOCK_PORT ?? 7338}
// Token "dev". Fixture data only; no daemon, no secrets. Tasks 8-10 extend FIXTURES.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../e2e/package.json', import.meta.url));
const { WebSocketServer } = require('ws');

const WEB = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PORT = Number(process.env.MOCK_PORT ?? 7338);
const TOKEN = 'dev';
const VERSION = '0.8.0-mock';

// ---- CSP, with the import map hashed like build.rs ----
function importMapHash() {
  const html = fs.readFileSync(path.join(WEB, 'index.html'), 'utf8');
  const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
  return m ? crypto.createHash('sha256').update(m[1]).digest('base64') : null;
}
function csp() {
  const h = importMapHash();
  return `default-src 'self'; script-src 'self'${h ? ` 'sha256-${h}'` : ''}; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`;
}

// ---- fixtures ----
const now = () => Math.floor(Date.now() / 1000);
const uuid = () => crypto.randomUUID();
const ids = {
  approve: '11111111-1111-4111-8111-111111111111',
  elicit: '22222222-2222-4222-8222-222222222222',
  input: '33333333-3333-4333-8333-333333333333',
  working: '44444444-4444-4444-8444-444444444444',
  idle: '55555555-5555-4555-8555-555555555555',
  shell: '66666666-6666-4666-8666-666666666666',
};

function session(id, o) {
  return {
    id, host: 'local', kind: 'claude', cwd: '/home/dev/claudio', name: null, title: null, label: 'claudio',
    state: 'idle', state_since: now() - 120, attention: null, claude_session_id: uuid(),
    model: 'opus', context_tokens: 82000, output_tokens: 12000, turns: 7, branch: 'main',
    git: { head: 'main', detached: false, upstream: true, ahead: 0, behind: 0, staged: 0, unstaged: 0, untracked: 0, conflicts: 0 },
    created_at: now() - 3600, pid: 4242, dormant: false, proxy: null, proxy_known: true, first_party: false,
    ephemeral: false, prompt_pending: false, ...o,
  };
}

const state = {
  hosts: {
    local: { name: 'local', connected: true, connecting: false, error: null, outdated: false, hostname: 'devbox', os: 'linux', arch: 'x86_64',
      claude: { path: '/usr/local/bin/claude', version: '2.1.0' }, features: ['lens', 'search', 'last_prompt', 'attention', 'worktree'], version: VERSION },
    z6: { name: 'z6', connected: true, connecting: false, error: null, outdated: false, hostname: 'z6', os: 'linux', arch: 'aarch64',
      claude: { path: '/home/p4u/.local/bin/claude', version: '2.0.9' }, features: ['lens', 'search', 'last_prompt'], version: VERSION },
  },
  sessions: new Map(),
  order: [],
  layout: { grid: 1, panes: [null], focus: 0 },
  profiles: { names: ['work', 'personal'], default: 'work' },
  extensions: [
    { name: 'latex', description: 'Renders <latex> formulas', enabled: true, source: 'builtin', profile: null, refused: null, lenses: [{ name: 'latex', kind: 'tag' }], actions: [] },
    { name: 'search', description: 'Search past conversations', enabled: true, source: 'builtin', profile: null, refused: null, lenses: [], actions: [{ id: 'search', title: 'Search', input: 'query' }] },
    { name: 'snippets', description: 'Prompt snippets', enabled: true, source: 'builtin', profile: null, refused: null, lenses: [], actions: [] },
  ],
  snippets: [{ name: 'review', text: 'Review the diff for bugs.' }],
  plugins: [{ name: 'vocdoni-internal', version: '1.2.0', profile: 'work', enabled: true, refused: null, choices_saved: false, manifest: 'ready', manifest_error: null, description: 'Internal tools' }],
  seq: new Map(),
};

function seed() {
  const add = (s) => { state.sessions.set(s.id, s); state.order.push(s.id); };
  add(session(ids.approve, { label: 'api-server', cwd: '/home/dev/api', branch: 'feat/auth', state: 'needs_approval', state_since: now() - 42,
    git: { head: 'feat/auth', detached: false, upstream: true, ahead: 2, behind: 0, staged: 1, unstaged: 3, untracked: 0, conflicts: 0 },
    attention: { seq: 3, kind: 'permission_prompt', message: 'Claude needs your permission to use Bash', tool: 'Bash', detail: 'cargo test --workspace -- --nocapture' } }));
  add(session(ids.elicit, { label: 'docs-site', cwd: '/home/dev/docs', branch: 'main', state: 'needs_approval', state_since: now() - 15, model: 'sonnet',
    attention: { seq: 1, kind: 'elicitation_dialog', message: 'Which package manager should I use?', tool: null, detail: null } }));
  add(session(ids.input, { label: 'claudio', state: 'needs_input', state_since: now() - 300, attention: { seq: 2, kind: 'idle_prompt', message: 'Claude is waiting for your input', tool: null, detail: null } }));
  add(session(ids.working, { label: 'parser', cwd: '/home/dev/parser', branch: 'refactor/lexer', state: 'working', state_since: now() - 8, context_tokens: 151000,
    git: { head: 'refactor/lexer', detached: false, upstream: false, ahead: 0, behind: 0, staged: 0, unstaged: 6, untracked: 2, conflicts: 0 } }));
  add(session(ids.idle, { label: 'infra', host: 'z6', cwd: '/srv/infra', branch: 'main', state: 'idle', state_since: now() - 1800, model: 'sonnet', proxy: 'work', first_party: false }));
  add(session(ids.shell, { kind: 'shell', label: 'term', cwd: '/home/dev/claudio', state: 'idle', model: null, branch: null, git: null, claude_session_id: null, context_tokens: null, turns: null, output_tokens: null }));
}
seed();

const snapshot = () => ({
  t: 'snapshot', hosts: Object.values(state.hosts), sessions: [...state.sessions.values()],
  profiles: state.profiles, layout: state.layout, order: state.order,
});

// ---- clients and broadcast ----
const clients = new Set();
const send = (ws, o) => ws.readyState === 1 && ws.send(JSON.stringify(o));
const ev = (name, fields) => ({ t: 'ev', ev: name, ...fields });
const broadcast = (o, except) => { for (const c of clients) if (c.ws !== except) send(c.ws, o); };
const emitSession = (s) => broadcast(ev('session', { session: s }));

function frame(kind, sid, payload) {
  const idb = Buffer.from(sid.replace(/-/g, ''), 'hex');
  return Buffer.concat([Buffer.from([kind]), idb, Buffer.isBuffer(payload) ? payload : Buffer.from(payload)]);
}

const BANNER = (s) => `\x1b[2J\x1b[H\x1b[1;36mclaudio mock\x1b[0m  ${s.label} @ ${s.host}\r\n\x1b[2mThis is a fixture terminal: what you type is echoed back.\x1b[0m\r\n\r\n$ `;

// ---- ops ----
class OpError extends Error {}
const bad = (m) => new OpError(`bad request: ${m}`);
const need = (a, k, type = 'string') => {
  if (typeof a[k] !== type) throw bad(`${k} must be a ${type}`);
  return a[k];
};
const sess = (a) => {
  const s = state.sessions.get(need(a, 'sid'));
  if (!s) throw new OpError('no such session');
  return s;
};
const host = (a) => {
  const h = state.hosts[need(a, 'host')];
  if (!h) throw new OpError('no such host');
  if (!h.connected) throw new OpError('not connected');
  return h;
};
const needHostFeature = (h, f) => {
  if (!h.features.includes(f)) throw new OpError("unsupported by this host's daemon");
};

function newSession(h, kind, a) {
  const id = uuid();
  const s = session(id, {
    host: h.name, kind, cwd: a.cwd, name: a.name ?? null,
    label: a.name ?? (kind === 'shell' ? 'term' : a.cwd.split('/').filter(Boolean).pop() ?? a.cwd),
    state: 'starting', state_since: now(), created_at: now(), model: null, context_tokens: null, turns: null, output_tokens: null,
    branch: null, git: null, claude_session_id: null, prompt_pending: !!a.prompt, proxy: a.proxy?.kind === 'profile' ? a.proxy.name : null,
  });
  state.sessions.set(id, s);
  state.order.push(id);
  emitSession(s);
  setTimeout(() => {
    if (!state.sessions.has(id)) return;
    s.state = 'idle'; s.state_since = now(); s.prompt_pending = false;
    emitSession(s);
  }, 700);
  return id;
}

const GIT_STATUS = {
  root: '/home/dev/claudio', branch: 'main', truncated: false,
  files: [
    { path: 'src/main.rs', old_path: null, index: 'M', worktree: '.', added: 4, removed: 1 },
    { path: 'web/app/store.js', old_path: null, index: '.', worktree: 'M', added: 21, removed: 7 },
    { path: 'notes.txt', old_path: null, index: '?', worktree: '?', added: null, removed: null },
  ],
};
const PATCH = (p) => ({ text: `diff --git a/${p} b/${p}\n--- a/${p}\n+++ b/${p}\n@@ -1,2 +1,3 @@\n context\n-old line\n+new line\n+another line\n`, truncated: false, binary: false });
const COMMIT = (oid) => ({ oid, short: oid.slice(0, 7), author: 'Dev', time: now() - 3600, subject: 'feat: fixture commit', body: '', files: [{ path: 'src/main.rs', added: 4, removed: 1 }], truncated: false });

const OPS = {
  'hosts.list': () => ({ hosts: Object.values(state.hosts) }),
  'hosts.candidates': () => ({ hosts: ['build-box', 'edge-1'] }),
  'hosts.connect': (a) => {
    const name = need(a, 'host');
    if (name === 'local') throw new OpError('local is always connected');
    if (!state.hosts[name]) {
      state.hosts[name] = { name, connected: true, connecting: false, error: null, outdated: false, hostname: name, os: 'linux', arch: 'x86_64', claude: { path: '/usr/bin/claude', version: '2.1.0' }, features: ['lens', 'search'], version: VERSION };
      broadcast(ev('host', { host: state.hosts[name] }));
    }
    return { host: state.hosts[name] };
  },
  'hosts.disconnect': (a) => {
    const name = need(a, 'host');
    if (name === 'local') throw new OpError('cannot disconnect local');
    if (!state.hosts[name]) throw new OpError('no such host');
    for (const s of [...state.sessions.values()].filter((x) => x.host === name)) { state.sessions.delete(s.id); broadcast(ev('session_removed', { sid: s.id })); }
    delete state.hosts[name];
    broadcast(ev('host_removed', { name }));
    return {};
  },
  'session.attach': (a, c) => {
    const s = sess(a);
    const rows = need(a, 'rows', 'number'); const cols = need(a, 'cols', 'number');
    if (rows < 1 || rows > 1000 || cols < 1 || cols > 1000) throw bad('rows/cols out of range');
    c.attached.add(s.id);
    setImmediate(() => {
      send(c.ws, ev('attached', { sid: s.id, rows, cols }));
      c.ws.send(frame(0x01, s.id, BANNER(s)));
    });
    return {};
  },
  'session.detach': (a, c) => { c.attached.delete(sess(a).id); return {}; },
  'session.resize': (a) => { sess(a); need(a, 'rows', 'number'); need(a, 'cols', 'number'); return {}; },
  'session.spawn': (a) => {
    const h = host(a);
    need(a, 'cwd'); need(a, 'rows', 'number'); need(a, 'cols', 'number');
    if (!a.proxy || !['default', 'direct', 'profile'].includes(a.proxy.kind)) throw bad('proxy');
    if (a.first_party && a.proxy.kind === 'direct') throw new OpError('first-party needs a proxy profile');
    if (typeof a.prompt === 'string' && a.prompt.length > 65536) throw new OpError('too large');
    return { sid: newSession(h, 'claude', a) };
  },
  'session.shell': (a) => { const h = host(a); need(a, 'cwd'); need(a, 'rows', 'number'); need(a, 'cols', 'number'); return { sid: newSession(h, 'shell', a) }; },
  'session.respawn': (a) => {
    const s = sess(a);
    need(a, 'fresh', 'boolean'); need(a, 'rows', 'number'); need(a, 'cols', 'number');
    if (s.kind === 'claude' && !a.proxy && !s.proxy_known) throw new OpError('proxy unknown');
    s.state = 'starting'; s.state_since = now(); s.attention = null; s.dormant = false; s.pid = 5000;
    emitSession(s);
    setTimeout(() => { s.state = 'idle'; emitSession(s); }, 700);
    return { sid: s.id };
  },
  'session.kill': (a) => {
    const s = sess(a);
    state.sessions.delete(s.id);
    state.order = state.order.filter((x) => x !== s.id);
    broadcast(ev('session_removed', { sid: s.id }));
    return {};
  },
  'session.rename': (a) => {
    const s = sess(a);
    if (a.name !== null && (typeof a.name !== 'string' || a.name.length > 200)) throw bad('name');
    s.name = a.name; s.label = a.name ?? s.cwd.split('/').filter(Boolean).pop() ?? 'term';
    emitSession(s);
    return {};
  },
  'session.paste': (a) => {
    sess(a);
    if (typeof a.text !== 'string') throw bad('text must be a string');
    if (a.text.length > 65536) throw new OpError('too large');
    need(a, 'submit', 'boolean');
    return {};
  },
  'session.approve': (a) => {
    const s = sess(a);
    if (!['approve', 'deny'].includes(a.decision)) throw bad('decision');
    const seq = need(a, 'attention_seq', 'number');
    const h = state.hosts[s.host];
    if (!h.features.includes('attention')) { if (seq !== 0 || s.state !== 'needs_approval') throw new OpError('stale'); }
    else {
      if (s.state !== 'needs_approval' || !s.attention || s.attention.seq !== seq) throw new OpError('stale');
      if (!['permission_prompt', 'worker_permission_prompt'].includes(s.attention.kind)) throw new OpError('not a permission prompt');
    }
    s.state = 'working'; s.state_since = now(); s.attention = null;
    emitSession(s);
    return {};
  },
  'session.order': (a, c) => {
    if (!Array.isArray(a.sids)) throw bad('sids');
    state.order = a.sids.filter((x) => state.sessions.has(x));
    broadcast(ev('order', { sids: state.order }), c.ws);
    return {};
  },
  'fs.list': (a) => {
    host(a);
    const p = need(a, 'path');
    return { path: p, truncated: false, entries: [
      { name: 'src', dir: true, git: null, claude_at: null, symlink: false, hidden: false },
      { name: 'web', dir: true, git: null, claude_at: now() - 86400, symlink: false, hidden: false },
      { name: '.git', dir: true, git: null, claude_at: null, symlink: false, hidden: true },
      { name: 'Cargo.toml', dir: false, git: null, claude_at: null, symlink: false, hidden: false },
    ] };
  },
  'fs.projects': (a) => {
    host(a);
    return { dirs: [
      { path: '/home/dev/claudio', modified: now() - 600, git: 'main', symlink: false, hidden: false },
      { path: '/home/dev/api', modified: now() - 7200, git: 'feat/auth', symlink: false, hidden: false },
      { path: '/home/dev/docs', modified: now() - 86400, git: null, symlink: false, hidden: false },
    ] };
  },
  'claude.sessions': (a) => {
    host(a);
    return { cwd: need(a, 'cwd'), sessions: [
      { id: uuid(), title: 'Refactor the lexer', last_prompt: 'split tokens into a module', modified: now() - 900, messages: 24 },
      { id: uuid(), title: 'Fix flaky test', last_prompt: 'why does it fail on CI', modified: now() - 90000, messages: 8 },
    ] };
  },
  'reader.answers': (a) => {
    sess(a);
    return { answers: [
      { prompt: 'Explain the energy formula', text: '# Energy\n\nThe famous relation is <latex>E = mc^2</latex>.\n\n```mermaid\ngraph TD; A-->B;\n```\n\n```rust\nfn main() {}\n```\n' },
      { prompt: 'List the steps', text: '1. Build\n2. Test\n3. Ship\n' },
    ] };
  },
  'reader.last_prompt': (a) => { sess(a); return { prompt: 'split tokens into a module', cut: false }; },
  search: (a) => {
    need(a, 'query');
    return { failed: [], truncated: false, hits: [
      { host: 'local', conversation: uuid(), cwd: '/home/dev/claudio', modified: now() - 3000, title: 'Refactor the lexer', snippet: `…the ${a.query} appears here…`, matches: 2 },
    ] };
  },
  'git.status': (a) => { const s = sess(a); needHostFeature(state.hosts[s.host], 'worktree'); return GIT_STATUS; },
  'git.worktree_diff': (a) => {
    const s = sess(a); needHostFeature(state.hosts[s.host], 'worktree');
    const p = need(a, 'path'); need(a, 'staged', 'boolean');
    if (!GIT_STATUS.files.some((f) => f.path === p)) throw new OpError('no such path');
    return PATCH(p);
  },
  'git.log': (a) => {
    sess(a);
    return { commits: [COMMIT('a'.repeat(40)), COMMIT('b'.repeat(40))].map(({ files, body, ...c }) => c), next_skip: null, truncated: false };
  },
  'git.commit': (a) => { sess(a); return COMMIT(need(a, 'oid')); },
  'git.diff': (a) => { sess(a); need(a, 'oid'); return PATCH('src/main.rs'); },
  'proxy.stats': (a) => ({
    profile: need(a, 'profile'), window: need(a, 'window'), user_name: 'dev',
    totals: { requests: 412, errors: 3, input_tokens: 1200000, output_tokens: 310000, cache_read: 9000000, cache_creation: 400000 },
    by_model: [{ model: 'opus', requests: 300, errors: 2, input_tokens: 900000, output_tokens: 250000, cache_read: 7000000, cache_creation: 300000 }],
    limit: { output_tokens: 1000000, window_seconds: 18000, used_output_tokens: 310000, used_pct: 31, blocked: false, blocked_until: null },
    pool: [{ name: 'acct-1', status: 'ok' }],
  }),
  'proxy.credential': (a) => {
    const s = sess(a);
    if (!s.proxy) throw new OpError('no proxy for this session');
    return { credential: { name: 'acct-1', plan: 'max', provider: 'anthropic', five_hour_pct: 41, seven_day_pct: 12, switched_secs_ago: null } };
  },
  'proxy.switch': (a) => {
    const s = sess(a);
    if (!s.proxy) throw new OpError('no proxy for this session');
    return { from: 'acct-1', to: 'acct-2', state: 'switched' };
  },
  'snippets.list': () => ({ snippets: state.snippets }),
  'snippets.save': (a) => { const text = need(a, 'text'); const name = `snippet-${state.snippets.length + 1}`; state.snippets.push({ name, text }); return { name }; },
  'extensions.list': () => ({ extensions: state.extensions }),
  'extensions.toggle': (a) => {
    const e = state.extensions.find((x) => x.name === need(a, 'name'));
    if (!e) throw new OpError('no such extension');
    e.enabled = need(a, 'on', 'boolean');
    return {};
  },
  'extensions.action': (a) => {
    const e = state.extensions.find((x) => x.name === need(a, 'ext'));
    if (!e) throw new OpError('no such extension');
    if (e.source === 'builtin') throw new OpError('not an external extension');
    return { result: { kind: 'notice', text: 'done' } };
  },
  'extensions.render': (a) => { need(a, 'lens'); need(a, 'kind'); need(a, 'source'); return { inline: null, block: null }; },
  'plugins.list': () => ({ plugins: state.plugins }),
  'plugins.items': (a) => {
    const p = state.plugins.find((x) => x.name === need(a, 'name'));
    if (!p) throw new OpError('no such plugin');
    return { name: p.name, version: p.version, items: [
      { id: `plugin:${p.name}@market`, section: 'Plugins', title: p.name, checked: true },
      { id: 'mcp:docs', section: 'MCP servers', title: 'docs', checked: true },
      { id: 'agent:reviewer', section: 'Agents', title: 'reviewer', checked: false },
    ] };
  },
  'plugins.save': (a) => {
    const p = state.plugins.find((x) => x.name === need(a, 'name'));
    if (!p) throw new OpError('no such plugin');
    if (!Array.isArray(a.disabled)) throw bad('disabled');
    p.choices_saved = true;
    return {};
  },
  'plugins.status': () => ({ status: [{ host: 'local', name: 'vocdoni-internal', state: 'ready', installed: [], present: ['mcp:docs'], skipped: [], dropped: [], error: null }] }),
  'claude.update': (a) => { host(a); need(a, 'install', 'boolean'); return {}; },
  'claude.skip': (a) => { host(a); return {}; },
  'layout.save': (a, c) => {
    const l = a.layout;
    const ok = l && [1, 2, 4].includes(l.grid) && Array.isArray(l.panes) && l.panes.length === l.grid
      && Number.isInteger(l.focus) && l.focus >= 0 && l.focus < l.grid
      && l.panes.every((p) => p === null || state.sessions.has(p));
    if (!ok) throw bad('layout');
    state.layout = l;
    broadcast(ev('layout', { layout: l }), c.ws);
    return {};
  },
};

function handle(c, msg) {
  const reply = (o) => send(c.ws, { t: 'res', id: msg.id, ...o });
  if (c.inflight >= 64) return reply({ ok: false, error: 'busy' });
  const fn = OPS[msg.op];
  if (!fn) return reply({ ok: false, error: 'unknown op' });
  c.inflight++;
  try {
    reply({ ok: true, data: fn(msg, c) ?? {} });
  } catch (e) {
    if (e instanceof OpError) reply({ ok: false, error: e.message });
    else { console.error(e); reply({ ok: false, error: 'internal error' }); }
  } finally { c.inflight--; }
}

// ---- HTTP ----
const sessions = new Set();
const cookieOf = (req) => /(?:^|;\s*)claudio_web=([0-9a-f]{64})/.exec(req.headers.cookie ?? '')?.[1];
const live = (req) => sessions.has(cookieOf(req));
const hostOk = (req) => ['127.0.0.1', 'localhost', '[::1]'].some((h) => [`${h}:${PORT}`].includes((req.headers.host ?? '').toLowerCase()));
const originOk = (req) => {
  const o = req.headers.origin;
  return typeof o === 'string' && /^https?:\/\//.test(o) && hostOk({ headers: { host: o.replace(/^https?:\/\//, '') } });
};

const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.json': 'application/json', '.txt': 'text/plain', '.svg': 'image/svg+xml', '.map': 'application/json' };

function headers(extra = {}) {
  return { 'Content-Security-Policy': csp(), 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer', ...extra };
}
function end(res, code, extra = {}, body) { res.writeHead(code, headers(extra)); res.end(body); }

const failures = [];
function login(req, res) {
  const recent = failures.filter((t) => Date.now() - t < 60000);
  failures.length = 0; failures.push(...recent);
  if (recent.length >= 5) return end(res, 429, { 'Cache-Control': 'no-store' });
  let body = '';
  let size = 0;
  req.on('data', (d) => { size += d.length; if (size > 4096) { end(res, 413); req.destroy(); } else body += d; });
  req.on('end', () => {
    let tok;
    try { tok = JSON.parse(body).token; } catch { return end(res, 400); }
    if (typeof tok !== 'string') return end(res, 400);
    const a = Buffer.from(tok); const b = Buffer.from(TOKEN);
    if (a.length !== b.length || !crypto.timingSafeEqual(a, b)) { failures.push(Date.now()); return end(res, 401, { 'Cache-Control': 'no-store' }); }
    const cookie = crypto.randomBytes(32).toString('hex');
    sessions.add(cookie);
    end(res, 204, { 'Set-Cookie': `claudio_web=${cookie}; HttpOnly; SameSite=Strict; Path=/`, 'Cache-Control': 'no-store' });
  });
}

const server = http.createServer((req, res) => {
  if (!hostOk(req)) return end(res, 421);
  const url = new URL(req.url, 'http://x');
  const isRead = req.method === 'GET' || req.method === 'HEAD';
  if (!isRead && !originOk(req)) return end(res, 403);
  if (url.pathname === '/api/login' && req.method === 'POST') return login(req, res);
  if (url.pathname.startsWith('/api/') || url.pathname === '/ws') {
    if (!live(req)) return end(res, 401, { 'Cache-Control': 'no-store' });
    if (url.pathname === '/api/me') return end(res, 204, { 'Cache-Control': 'no-store' });
    if (url.pathname === '/api/logout' && req.method === 'POST') {
      sessions.delete(cookieOf(req));
      return end(res, 204, { 'Set-Cookie': 'claudio_web=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0', 'Cache-Control': 'no-store' });
    }
    return end(res, 404, { 'Cache-Control': 'no-store' });
  }
  if (!isRead) return end(res, 404);
  const rel = url.pathname === '/' ? '/index.html' : decodeURIComponent(url.pathname);
  const file = path.resolve(WEB, `.${rel}`);
  if (!file.startsWith(WEB + path.sep) || file.includes(`${path.sep}dev${path.sep}`) || file.includes(`${path.sep}e2e${path.sep}`) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return end(res, 404);
  const data = fs.readFileSync(file);
  const etag = `"${crypto.createHash('sha1').update(data).digest('hex')}"`;
  const cache = rel === '/index.html' ? 'no-store' : 'no-cache';
  if (req.headers['if-none-match'] === etag) return end(res, 304, { ETag: etag, 'Cache-Control': cache });
  end(res, 200, { 'Content-Type': MIME[path.extname(file)] ?? 'application/octet-stream', ETag: etag, 'Cache-Control': cache }, req.method === 'HEAD' ? undefined : data);
});

// ---- WebSocket ----
const wss = new WebSocketServer({ noServer: true, maxPayload: 1024 * 1024 });
server.on('upgrade', (req, socket, head) => {
  const url = new URL(req.url, 'http://x');
  const reject = (code) => { socket.write(`HTTP/1.1 ${code} X\r\nConnection: close\r\n\r\n`); socket.destroy(); };
  if (!hostOk(req)) return reject(421);
  if (!originOk(req)) return reject(403);
  if (url.pathname !== '/ws' || !live(req)) return reject(401);
  if (clients.size >= 16) return reject(503);
  wss.handleUpgrade(req, socket, head, (ws) => wss.emit('connection', ws));
});

wss.on('connection', (ws) => {
  const c = { ws, attached: new Set(), inflight: 0 };
  clients.add(c);
  send(ws, { t: 'hello', v: 1, version: VERSION, features: [] });
  send(ws, snapshot());
  for (const h of Object.values(state.hosts)) send(ws, ev('host_stats', { host: h.name, cpu: 12, mem_used: 6e9, mem_total: 16e9 }));
  ws.on('message', (data, isBinary) => {
    if (isBinary) {
      const b = Buffer.from(data);
      if (b.length < 17 || b[0] !== 0x02) return ws.close(1003);
      const sid = [...b.subarray(1, 17)].map((x) => x.toString(16).padStart(2, '0')).join('').replace(/^(.{8})(.{4})(.{4})(.{4})(.{12})$/, '$1-$2-$3-$4-$5');
      const s = state.sessions.get(sid);
      if (!s || !c.attached.has(sid)) return;
      const echo = Buffer.from(b.subarray(17).toString('latin1').replace(/\r/g, '\r\n$ '), 'latin1');
      ws.send(frame(0x01, sid, echo));
      return;
    }
    if (data.length > 65536) return ws.close(1009);
    let msg;
    try { msg = JSON.parse(data.toString()); } catch { return ws.close(1003); }
    if (!msg || msg.t !== 'req' || !Number.isInteger(msg.id)) return ws.close(1003);
    handle(c, msg);
  });
  ws.on('close', () => clients.delete(c));
});

const statsTimer = setInterval(() => {
  for (const h of Object.values(state.hosts)) {
    broadcast(ev('host_stats', { host: h.name, cpu: 8 + Math.round(Math.random() * 30), mem_used: 6e9, mem_total: 16e9 }));
  }
}, 2000);
statsTimer.unref();

server.listen(PORT, '127.0.0.1', () => {
  console.log(`mock: http://127.0.0.1:${PORT}/#t=${TOKEN}`);
});
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => process.exit(0));
