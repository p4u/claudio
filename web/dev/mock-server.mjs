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
    z6: { name: 'z6', connected: true, connecting: false, error: null, outdated: true, hostname: 'z6', os: 'linux', arch: 'aarch64',
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
    { name: 'vocdoni-internal', description: 'Internal tools: docs MCP server, reviewer agent', enabled: true, source: 'plugin', profile: 'work', refused: null, lenses: [], actions: [] },
    { name: 'broken-ext', description: 'An external extension that failed admission', enabled: false, source: 'external', profile: null, refused: 'manifest: lens name "todo" is already taken by tasks', lenses: [], actions: [] },
    { name: 'tasks', description: 'Task notes (fixture extension)', enabled: true, source: 'external', profile: null, refused: null, lenses: [{ name: 'todo', kind: 'tag' }],
      actions: [{ id: 'summary', title: 'Summary document', input: null }, { id: 'note', title: 'Add a note', input: 'note text' }, { id: 'pick', title: 'Pick a task', input: null }, { id: 'insert', title: 'Insert checklist', input: null }, { id: 'resume', title: 'Resume the last chat', input: null }] },
  ],
  snippets: [{ name: 'review', text: 'Review the diff for bugs.' }, { name: 'explain', text: 'Explain this code as if I were new to the project.\nName the files you read.' }],
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
  add(session(ids.working, { label: 'parser', cwd: '/home/dev/parser', branch: 'refactor/lexer', proxy: 'work', state: 'working', state_since: now() - 8, context_tokens: 151000,
    git: { head: 'refactor/lexer', detached: false, upstream: false, ahead: 0, behind: 0, staged: 0, unstaged: 6, untracked: 2, conflicts: 0 } }));
  add(session(ids.idle, { label: 'infra', host: 'z6', cwd: '/srv/infra', branch: 'main', state: 'idle', state_since: now() - 1800, model: 'sonnet', proxy: 'work', first_party: false }));
  add(session(ids.shell, { kind: 'shell', label: 'term', cwd: '/home/dev/claudio', state: 'idle', model: null, branch: null, git: null, claude_session_id: null, context_tokens: null, turns: null, output_tokens: null }));
}
seed();

const snapshot = () => ({
  t: 'snapshot', hosts: Object.values(state.hosts), sessions: [...state.sessions.values()],
  profiles: state.profiles, layout: state.layout, order: state.order,
});


// ---- reader fixtures (Task 9): a rich answer, an XSS answer that must render inert, a plain one ----
const RICH_ANSWER = [
  '## Why the rotation matrix preserves length',
  '',
  'A rotation by an angle <latex>\\theta</latex> acts on the plane through the matrix below. Because its columns are orthonormal, every vector keeps its length: the norm squared is <latex>\\lVert Rv \\rVert^2 = v^\\top R^\\top R\\, v = \\lVert v \\rVert^2</latex>, which is the whole argument.',
  '',
  '<latex>R(\\theta) = \\begin{pmatrix} \\cos\\theta & -\\sin\\theta \\\\ \\sin\\theta & \\cos\\theta \\end{pmatrix}</latex>',
  '',
  'The same idea gives the Gaussian integral, a classic display equation:',
  '',
  '<latex>\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}</latex>',
  '',
  '### How the pieces fit',
  '',
  '```mermaid',
  'flowchart LR',
  '  A[Prompt] --> B{Needs a tool?}',
  '  B -- yes --> C[Run the tool]',
  '  B -- no --> D[Answer]',
  '  C --> D',
  '```',
  '',
  '| Layer | Owns | Notes |',
  '|---|---|---|',
  '| Daemon | PTYs, journal | one per host |',
  '| Hub | attach refcount, fan-out | in `claudio web` |',
  '| Browser | xterm, reader | no build step |',
  '',
  'Parsing a line in Rust looks like this:',
  '',
  '```rust',
  'fn parse(line: &str) -> Result<u32, std::num::ParseIntError> {',
  '    // trim first, then parse',
  '    line.trim().parse::<u32>()',
  '}',
  '```',
  '',
  '```python',
  'def norm2(v):',
  '    return sum(x * x for x in v)',
  '```',
  '',
  '- Rotations are **isometries**.',
  '- Their determinant is `+1`, unlike reflections.',
  '- See [the notes](https://example.com/rotations) for more.',
  '',
  '> Tags inside code stay text: `<latex>x</latex>`.',
  '',
].join('\n');

const XSS_ANSWER = [
  'This answer is a hostile fixture. Nothing below may run.',
  '',
  '<img src=x onerror=alert(1)>',
  '',
  '<script>window.__pwned = 1</script>',
  '',
  '[click me](javascript:alert(1))',
  '',
  '<a href="javascript:alert(2)" onclick="alert(3)">second link</a>',
  '',
  '<iframe src="https://example.com"></iframe><style>body{display:none}</style>',
  '',
  '<latex>\\href{javascript:alert(1)}{x}</latex> and <latex>\\unknownmacro{</latex>',
  '',
  'Forged placeholder: \uE000deadbeef:0\uE001 and x:0',
  '',
  '```mermaid',
  'graph TD',
  '  A[Start] --> B[End]',
  '  click A href "javascript:alert(1)"',
  '  click B call alert(1)',
  '```',
  '',
  '```mermaid',
  'notADiagramType',
  '  x --> y',
  '```',
  '',
].join('\n');

const ANSWERS = [
  { prompt: 'Explain why a rotation matrix preserves length, show the matrix and a diagram of the flow.\nKeep it short.', text: RICH_ANSWER },
  { prompt: 'Show me something hostile', text: XSS_ANSWER },
  { prompt: 'List the steps', text: '1. Build\n2. Test\n3. Ship\n' },
];
const LENS = [
  { tag: 'latex', text: 'E = mc^2' },
  { tag: 'latex', text: '\\int_0^1 x^2\\,dx = \\tfrac{1}{3}' },
  { tag: 'todo', text: 'wire the reader to the lens feed' },
];

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

// Write text into the terminal of every client attached to a session (how the mock shows what an op did).
function toTerminals(sid, text) {
  for (const c of clients) if (c.attached.has(sid)) c.ws.send(frame(0x01, sid, text));
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

// Shapes follow src/proto.rs (GitWorktreeStatus, GitPatch, GitLogPage, GitCommitInfo): docs/web-protocol.md section 4.
const GIT_STATUS = {
  root: '/home/dev/claudio', branch: 'feat/auth', truncated: false,
  files: [
    { path: 'src/auth/session.rs', old_path: null, index: 'U', worktree: 'U', added: null, removed: null },
    { path: 'src/main.rs', old_path: null, index: 'M', worktree: '.', added: 6, removed: 2 },
    { path: 'src/lexer/mod.rs', old_path: null, index: 'M', worktree: 'M', added: 9, removed: 4 },
    { path: 'web/app/store.js', old_path: null, index: '.', worktree: 'M', added: 7, removed: 4 },
    { path: 'docs/guide.md', old_path: 'docs/intro.md', index: 'R', worktree: '.', added: 2, removed: 1 },
    { path: 'assets/logo.png', old_path: null, index: '.', worktree: 'M', added: null, removed: null },
    { path: 'notes.txt', old_path: null, index: '?', worktree: '?', added: null, removed: null },
    { path: 'scratch/todo.md', old_path: null, index: '?', worktree: '?', added: null, removed: null },
  ],
};
const MAIN_RS = [
  '@@ -1,12 +1,16 @@',
  ' use std::io;',
  '+use std::path::PathBuf;',
  ' ',
  ' mod auth;',
  ' mod lexer;',
  ' ',
  '-fn main() {',
  '-    let args: Vec<String> = std::env::args().collect();',
  '+fn main() -> io::Result<()> {',
  '+    let args: Vec<String> = std::env::args().skip(1).collect();',
  '+    let root = PathBuf::from(args.first().map(String::as_str).unwrap_or("."));',
  '     println!("starting");',
  '+    auth::init(&root)?;',
  '+    Ok(())',
  ' }',
  ' ',
  '@@ -40,6 +44,7 @@ fn helper() {',
  '     let n = 1;',
  '     let m = 2;',
  '-    println!("{}", n + m);',
  '+    println!("sum = {}", n + m);',
  '+    println!("product = {}", n * m);',
  '     done();',
  ' }',
].join('\n');
const LEXER_RS = [
  '@@ -10,9 +10,14 @@ impl Lexer {',
  '     pub fn next(&mut self) -> Token {',
  '         self.skip_ws();',
  '-        match self.peek() {',
  '-            Some(c) if c.is_ascii_digit() => self.number(),',
  '-            Some(c) if c.is_alphabetic() => self.ident(),',
  '-            _ => Token::Eof,',
  '-        }',
  '+        let Some(c) = self.peek() else { return Token::Eof };',
  '+        if c.is_ascii_digit() {',
  '+            self.number()',
  '+        } else if c.is_alphabetic() || c == \'_\' {',
  '+            self.ident()',
  '+        } else {',
  '+            self.punct(c)',
  '+        }',
  '     }',
  ' }',
].join('\n');
const STORE_JS = [
  '@@ -52,10 +52,10 @@ function handleEvent(ev) {',
  '   const before = ev.ev === \'session\' ? model.value.sessions[ev.session.id] : null;',
  '   model.value = applyEvent(model.value, ev);',
  '-  if (ev.ev === \'session\' && asking(ev.session)) markFresh(ev.session.id);',
  '+  if (ev.ev === \'session\' && asking(ev.session) && !(asking(before) && before.state === ev.session.state)) markFresh(ev.session.id);',
  '   if (ev.ev === \'notice\') toast({ text: ev.text, level: ev.level });',
  '-  for (const fn of listeners) fn(ev);',
  '+  for (const fn of listeners) fn(ev); // after the model update',
  ' }',
].join('\n');
const NEW_FILE = ['@@ -0,0 +1,3 @@', '+# Notes', '+', '+Remember to rotate the token.'].join('\n');
const header = (p, o = p) => `diff --git a/${o} b/${p}\n--- a/${o}\n+++ b/${p}\n`;
const WORKTREE_PATCH = {
  'src/main.rs': header('src/main.rs') + MAIN_RS + '\n',
  'src/lexer/mod.rs': header('src/lexer/mod.rs') + LEXER_RS + '\n',
  'web/app/store.js': header('web/app/store.js') + STORE_JS + '\n',
  'docs/guide.md': 'diff --git a/docs/intro.md b/docs/guide.md\nsimilarity index 80%\nrename from docs/intro.md\nrename to docs/guide.md\n--- a/docs/intro.md\n+++ b/docs/guide.md\n@@ -1,3 +1,4 @@\n # Guide\n-Old intro line.\n+New intro line.\n+A second line.\n ok\n',
  'assets/logo.png': 'diff --git a/assets/logo.png b/assets/logo.png\nindex 1111111..2222222 100644\nBinary files a/assets/logo.png and b/assets/logo.png differ\n',
  'notes.txt': 'diff --git a/notes.txt b/notes.txt\nnew file mode 100644\n--- /dev/null\n+++ b/notes.txt\n' + NEW_FILE + '\n',
  'scratch/todo.md': 'diff --git a/scratch/todo.md b/scratch/todo.md\nnew file mode 100644\n--- /dev/null\n+++ b/scratch/todo.md\n' + NEW_FILE + '\n',
  'src/auth/session.rs': 'diff --git a/src/auth/session.rs b/src/auth/session.rs\n--- a/src/auth/session.rs\n+++ b/src/auth/session.rs\n@@ -1,7 +1,13 @@\n fn load() {\n-    old();\n+<<<<<<< HEAD\n+    ours();\n+=======\n+    theirs();\n+>>>>>>> feat/other\n }\n',
};
const hex40 = (n) => crypto.createHash('sha1').update(`commit-${n}`).digest('hex');
const SUBJECTS = ['feat: add the session cookie', 'fix: reject an empty token', 'refactor: split the lexer', 'docs: explain the web ui', 'test: cover the approve guard', 'chore: bump dependencies', 'feat: queue cards show the tool', 'fix: keep the pty size on re-attach'];
const LOG_N = 230;
const logEntry = (i) => ({
  id: hex40(i), parents: i + 1 < LOG_N ? [hex40(i + 1)] : [], author: ['Pau Escrich', 'Dev', 'Ana Ruiz'][i % 3], time: now() - 1800 * (i + 1) - i * 977,
  refs: i === 0 ? ['HEAD -> refs/heads/feat/auth', 'refs/remotes/origin/feat/auth'] : i === 3 ? ['tag: refs/tags/v0.7.1'] : i === 5 ? ['refs/heads/main', 'refs/remotes/origin/main'] : [],
  subject: `${SUBJECTS[i % SUBJECTS.length]}${i >= SUBJECTS.length ? ` (${i})` : ''}`,
});
const COMMIT_FILES = [
  { path: 'src/main.rs', old_path: null, added: 6, removed: 2 },
  { path: 'src/lexer/mod.rs', old_path: null, added: 9, removed: 4 },
  { path: 'docs/guide.md', old_path: 'docs/intro.md', added: 2, removed: 1 },
  { path: 'assets/logo.png', old_path: null, added: null, removed: null },
];
const commitIndex = (oid) => { for (let i = 0; i < LOG_N; i++) if (hex40(i) === oid) return i; return -1; };
const COMMIT = (oid) => {
  const i = commitIndex(oid);
  if (i < 0) throw new OpError('no such commit');
  const e = logEntry(i);
  return { id: e.id, parents: e.parents, author: e.author, email: `${e.author.split(' ')[0].toLowerCase()}@example.com`, time: e.time, committer_time: e.time + 60, refs: e.refs,
    message: `${e.subject}\n\nThe body explains why. It wraps over a few lines and mentions the\nreader, the hub and the daemon so the layout can be judged.\n\n- first point\n- second point`, files: COMMIT_FILES, first_parent: i === 6 };
};
const COMMIT_PATCH = (file) => {
  const f = COMMIT_FILES[file];
  const body = { 'src/main.rs': MAIN_RS, 'src/lexer/mod.rs': LEXER_RS }[f.path];
  if (f.path === 'assets/logo.png') return 'diff --git a/assets/logo.png b/assets/logo.png\nBinary files a/assets/logo.png and b/assets/logo.png differ\n';
  if (f.path === 'docs/guide.md') return WORKTREE_PATCH['docs/guide.md'];
  return header(f.path) + body + '\n';
};

// Stats per window (the bars differ so a switch is visible).
const STATS = {
  '24h': { requests: 412, errors: 3, input: 1200000, output: 310000, cr: 9000000, cc: 400000, f: 1 },
  '7d': { requests: 2890, errors: 21, input: 8400000, output: 2150000, cr: 61000000, cc: 2800000, f: 7 },
  '30d': { requests: 11800, errors: 80, input: 33000000, output: 8900000, cr: 240000000, cc: 11000000, f: 30 },
};
const MODELS = [['claude-opus-4', 0.58], ['claude-sonnet-4', 0.31], ['claude-haiku-4', 0.09], ['claude-3-7-sonnet', 0.02]];
let credState = { name: 'acct-1', plan: 'max', five: 41, seven: 12 };
let retryAt = Date.now() + 45000;
const PLUGIN_STATUS = () => ([
  { host: 'local', name: 'vocdoni-internal', state: 'ready', installed: ['plugin:vocdoni-internal@market'], present: ['mcp:docs'], skipped: [{ id: 'agent:reviewer', reason: 'failed: marketplace unreachable', retry_secs: Math.max(0, Math.ceil((retryAt - Date.now()) / 1000)) }], dropped: [], error: null },
  { host: 'z6', name: 'vocdoni-internal', state: 'pending', installed: [], present: [], skipped: [{ id: 'mcp:docs', reason: 'missing secret DOCS_TOKEN', retry_secs: null }], dropped: ['--agents'], error: null },
]);
// ---- claude offers (replayed on connect, as the protocol says) ----
const claudeOffers = new Map([
  ['z6', { host: 'z6', phase: 'offer_update', have: '2.0.9', want: '2.1.0', version: null, tail: null }],
]);
let installFailedOnce = false;

const OPS = {
  'hosts.list': () => ({ hosts: Object.values(state.hosts) }),
  'hosts.candidates': () => ({ hosts: ['build-box', 'edge-1'] }),
  'hosts.connect': (a) => {
    const name = need(a, 'host');
    if (name === 'local') throw new OpError('local is always connected');
    if (name === 'edge-1') throw new OpError('ssh: connect to host edge-1 port 22: Connection timed out\nbootstrap failed');
    if (!state.hosts[name]) {
      state.hosts[name] = { name, connected: true, connecting: false, error: null, outdated: false, hostname: name, os: 'linux', arch: 'x86_64', claude: null, features: ['lens', 'search'], version: VERSION };
      broadcast(ev('host', { host: state.hosts[name] }));
      claudeOffers.set(name, { host: name, phase: 'offer_install', have: null, want: '2.1.0', version: null, tail: null });
      setTimeout(() => broadcast(ev('claude', claudeOffers.get(name))), 300);
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
    console.log(`mock: session.attach ${s.id.slice(0, 4)} ${cols}x${rows}`);
    setImmediate(() => {
      send(c.ws, ev('attached', { sid: s.id, rows, cols }));
      c.ws.send(frame(0x01, s.id, BANNER(s)));
    });
    return {};
  },
  'session.detach': (a, c) => { c.attached.delete(sess(a).id); return {}; },
  'session.resize': (a) => { sess(a); need(a, 'rows', 'number'); need(a, 'cols', 'number'); console.log(`mock: session.resize ${a.sid.slice(0, 4)} ${a.cols}x${a.rows}`); return {}; },
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
    setTimeout(() => { if (!state.sessions.has(s.id)) return; s.state = 'idle'; emitSession(s); }, 700);
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
    const s = sess(a);
    toTerminals(s.id, `\r\n\x1b[36m[mock] session.paste ${a.text.length} chars${a.submit ? ' + submit' : ''}:\x1b[0m ${a.text.replace(/\r?\n/g, '\r\n')}\r\n$ `);
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
    toTerminals(s.id, `\r\n\x1b[33m[mock] session.approve ${a.decision}: sent ${a.decision === 'approve' ? 'Enter (\\r)' : 'Esc (\\x1b)'}\x1b[0m\r\n$ `);
    console.log(`mock: session.approve ${a.decision} ${s.id} seq=${seq}`);
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
  'reader.answers': (a) => { sess(a); return { answers: ANSWERS }; },
  'reader.last_prompt': (a) => { sess(a); return { prompt: 'split tokens into a module', cut: false }; },
  search: (a) => {
    const q = need(a, 'query');
    const live = state.sessions.get(ids.working);
    return { failed: [{ host: 'z6', error: 'search busy' }], truncated: false, hits: [
      { host: 'local', conversation: live.claude_session_id, cwd: live.cwd, modified: now() - 1200, title: 'Refactor the lexer', snippet: `…we split the lexer so that ${q} is handled in one place…`, matches: 3 },
      { host: 'local', conversation: uuid(), cwd: '/home/dev/api', modified: now() - 86400 * 2, title: 'Fix flaky CI test', snippet: `…the flaky test mentions ${q} in its setup…`, matches: 1 },
      { host: 'z6', conversation: uuid(), cwd: '/srv/infra', modified: now() - 86400 * 9, title: 'Rotate the TLS certificates', snippet: `…certbot renew, then reload nginx (${q})…`, matches: 2 },
    ] };
  },
  'git.status': (a) => { const s = sess(a); needHostFeature(state.hosts[s.host], 'worktree'); return GIT_STATUS; },
  'git.worktree_diff': (a) => {
    const s = sess(a); needHostFeature(state.hosts[s.host], 'worktree');
    const p = need(a, 'path'); need(a, 'staged', 'boolean');
    if (!GIT_STATUS.files.some((f) => f.path === p)) throw new OpError('no such path');
    return { id: 'worktree', path: p, patch: WORKTREE_PATCH[p], truncated: p === 'src/lexer/mod.rs' && a.staged };
  },
  'git.log': (a) => {
    sess(a);
    const skip = a.skip ?? 0; const limit = Math.min(a.limit ?? 100, 500);
    if (!Number.isInteger(skip) || !Number.isInteger(limit)) throw bad('skip/limit');
    const total = a.all ? LOG_N + 40 : LOG_N;
    const commits = [];
    for (let i = skip; i < Math.min(total, skip + limit); i++) commits.push(i < LOG_N ? logEntry(i) : { ...logEntry(i), subject: `other branch: ${logEntry(i).subject}`, refs: i === LOG_N ? ['refs/heads/feat/other'] : [] });
    return { root: '/home/dev/claudio', head: 'feat/auth', commits, more: skip + commits.length < total };
  },
  'git.commit': (a) => { sess(a); return COMMIT(need(a, 'oid')); },
  'git.diff': (a) => {
    sess(a);
    const oid = need(a, 'oid');
    COMMIT(oid);
    if (a.file === undefined) return { id: oid, path: null, patch: COMMIT_FILES.map((_, i) => COMMIT_PATCH(i)).join(''), truncated: false };
    if (!Number.isInteger(a.file) || a.file < 0 || a.file >= COMMIT_FILES.length) throw new OpError('no such file');
    return { id: oid, path: COMMIT_FILES[a.file].path, patch: COMMIT_PATCH(a.file), truncated: false };
  },
  'proxy.stats': (a) => {
    const w = STATS[need(a, 'window')];
    if (!w) throw bad('window');
    const f = (v, share) => Math.round(v * share);
    const by_model = MODELS.map(([model, share]) => ({ model, requests: f(w.requests, share), errors: f(w.errors, share), input_tokens: f(w.input, share), output_tokens: f(w.output, share), cache_read: f(w.cr, share), cache_creation: f(w.cc, share) }));
    const limit = 1000000 * w.f;
    return {
      profile: need(a, 'profile'), window: a.window, user_name: 'dev',
      totals: { requests: w.requests, errors: w.errors, input_tokens: w.input, output_tokens: w.output, cache_read: w.cr, cache_creation: w.cc },
      by_model,
      limit: { output_tokens: limit, window_seconds: 18000, used_output_tokens: w.output, used_pct: Math.round((100 * w.output) / limit), blocked: false, blocked_until: null },
      pool: [{ name: 'acct-1', status: 'ok' }, { name: 'acct-2', status: 'ok' }, { name: 'acct-3', status: 'cooling down' }],
    };
  },
  'proxy.credential': (a) => {
    const s = sess(a);
    if (!s.proxy) throw new OpError('no proxy for this session');
    if (!s.claude_session_id) throw new OpError('no conversation yet');
    return { credential: { name: credState.name, plan: credState.plan, provider: 'anthropic', five_hour_pct: credState.five, seven_day_pct: credState.seven, switched_secs_ago: null } };
  },
  'proxy.switch': (a) => {
    const s = sess(a);
    if (!s.proxy) throw new OpError('no proxy for this session');
    if (!s.claude_session_id) throw new OpError('no conversation yet');
    const from = credState.name;
    credState = from === 'acct-1' ? { name: 'acct-2', plan: 'max', five: 88, seven: 63 } : { name: 'acct-1', plan: 'max', five: 41, seven: 12 };
    return { from, to: credState.name, state: 'switched' };
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
    switch (a.action) {
      case 'summary': return { result: { kind: 'document', title: 'Task summary', markdown: '# Tasks\n\nTwo open, one done. The estimate is <latex>T = \\sum_i t_i</latex>.\n\n| Task | State |\n|---|---|\n| Reader | done |\n| Wizard | open |\n' } };
      case 'note': return { result: { kind: 'notice', text: `Noted: ${a.input ?? ''}` } };
      case 'insert': return { result: { kind: 'paste', text: '- [ ] write tests\n- [ ] ship it' } };
      case 'resume': return { result: { kind: 'resume', host: 'local', cwd: '/home/dev/api', conversation: uuid() } };
      case 'pick': return { result: { kind: 'list', title: 'Tasks', items: [
        { label: 'Write the reader', detail: 'in progress', result: { kind: 'notice', text: 'Opened: Write the reader' } },
        { label: 'Resume the API chat', detail: '/home/dev/api', result: { kind: 'resume', host: 'local', cwd: '/home/dev/api', conversation: uuid() } },
      ] } };
      default: throw new OpError('no such action');
    }
  },
  'extensions.render': (a) => { const lens = need(a, 'lens'); need(a, 'kind'); const src = need(a, 'source'); return lens === 'todo' ? { inline: null, block: ['[ ] ' + src, '    (from the conversation)'] } : { inline: null, block: null }; },
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
  'plugins.status': () => ({ status: PLUGIN_STATUS().map(({ ...x }) => x) }),
  'claude.update': (a) => {
    const h = host(a); const install = need(a, 'install', 'boolean');
    const phase = install ? 'installing' : 'updating';
    claudeOffers.set(h.name, { host: h.name, phase, have: h.claude?.version ?? null, want: null, version: null, tail: null });
    broadcast(ev('claude', claudeOffers.get(h.name)));
    setTimeout(() => {
      let o;
      if (install && !installFailedOnce) { installFailedOnce = true; o = { host: h.name, phase: 'failed', have: null, want: null, version: null, tail: 'curl: (6) Could not resolve host: claude.ai\ninstaller exited with status 6' }; }
      else { h.claude = { path: '/home/dev/.local/bin/claude', version: '2.1.0' }; broadcast(ev('host', { host: h })); o = { host: h.name, phase: 'done', have: null, want: null, version: '2.1.0', tail: null }; }
      claudeOffers.set(h.name, o);
      broadcast(ev('claude', o));
    }, 2600);
    return {};
  },
  'claude.skip': (a) => { const h = host(a); claudeOffers.delete(h.name); return {}; },
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
  for (const sid of [ids.working, ids.idle]) {
    send(ws, ev('lens', { sid, seq: 1, items: LENS }));
    send(ws, ev('latex', { sid, seq: 1, formulas: LENS.filter((x) => x.tag === 'latex').map((x) => x.text) }));
  }
  for (const h of Object.values(state.hosts)) send(ws, ev('host_stats', { host: h.name, cpu: 12, mem_used: 6e9, mem_total: 16e9 }));
  for (const o of claudeOffers.values()) send(ws, ev('claude', o));
  for (const st of PLUGIN_STATUS()) send(ws, ev('plugin_status', st));
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

// Attention moves by itself: every 20 s the "parser" session, once it works again, asks for a new
// permission (a new seq), so the queue, the blink and the notifications can be seen live.
let attentionSeq = 10;
const cycler = setInterval(() => {
  const s = state.sessions.get(ids.working);
  if (!s || s.state !== 'working') return;
  s.state = 'needs_approval'; s.state_since = now();
  s.attention = { seq: ++attentionSeq, kind: 'permission_prompt', message: 'Claude needs your permission to use Edit', tool: 'Edit', detail: 'src/lexer/mod.rs' };
  emitSession(s);
}, 20000);
cycler.unref();

const statsTimer = setInterval(() => {
  for (const h of Object.values(state.hosts)) {
    broadcast(ev('host_stats', { host: h.name, cpu: 8 + Math.round(Math.random() * 30), mem_used: 6e9, mem_total: 16e9 }));
  }
}, 2000);
statsTimer.unref();

// The plugin retry comes due, then the daemon asks again and a new countdown starts.
const retryTimer = setInterval(() => {
  if (Date.now() < retryAt) return;
  retryAt = Date.now() + 45000;
  for (const st of PLUGIN_STATUS()) broadcast(ev('plugin_status', st));
}, 1000);
retryTimer.unref();

server.listen(PORT, '127.0.0.1', () => {
  console.log(`mock: http://127.0.0.1:${PORT}/#t=${TOKEN}`);
});
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => process.exit(0));
