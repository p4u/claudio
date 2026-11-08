// Starts a real `claudio web` (debug build) with a fake claude, in a scratch root under
// ~/.cache/claudio/web-e2e/<8 hex>/ and an env that cannot reach the user's own
// ~/.claude, config, or running daemon.
import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import WebSocket from 'ws';
import { startFakeProxy, PROXY_TOKEN } from './fake-proxy.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../../..');
const BIN = process.env.CLAUDIO_BIN ?? path.join(repo, 'target/debug/claudio');

const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
  s.on('error', reject);
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mangle = (cwd) => fs.realpathSync(cwd).replaceAll('/', '_');

/** The isolated env of the server (and of the daemon it starts). */
function isolatedEnv(root, extra) {
  const env = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('CLAUDIO_')) env[k] = v;
  for (const k of ['COLORTERM', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_BASE_URL']) delete env[k];
  return {
    ...env,
    XDG_RUNTIME_DIR: path.join(root, 'run'),
    XDG_CONFIG_HOME: path.join(root, 'cfg'),
    XDG_CACHE_HOME: path.join(root, 'xdg-cache'),
    HOME: path.join(root, 'home'),
    CLAUDE_CONFIG_DIR: path.join(root, 'claude-config'),
    CLAUDIO_CLAUDE_PATH: path.join(root, 'fake-claude'),
    CLAUDIO_NO_UPDATE_CHECK: '1',
    CLAUDIO_NO_BROWSER: '1',
    ANTHROPIC_API_KEY: 'test-key-not-real',
    TERM: 'xterm-256color',
    SHELL: '/bin/sh',
    FAKE_DIR: path.join(root, 'fake'),
    FAKE_DUMP_ENV: '1',
    CLAUDIO_TEST_LOOPBACK_HTTP: '1',
    ...extra,
  };
}

function git(cwd, ...args) {
  const r = spawnSync('git', ['-c', 'user.email=e2e@example.org', '-c', 'user.name=e2e', ...args], { cwd, encoding: 'utf8' });
  if (r.status !== 0) throw new Error(`git ${args.join(' ')}: ${r.stderr}`);
}

function postLogin(port, token) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({ token });
    const req = http.request({ host: '127.0.0.1', port, path: '/api/login', method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body), Origin: `http://127.0.0.1:${port}` } },
    (res) => { res.resume(); res.on('end', () => resolve(res)); });
    req.on('error', reject);
    req.end(body);
  });
}

/** A node WebSocket client, logged in like a browser, for setup and assertions. */
async function connectApi(port, token) {
  const res = await postLogin(port, token);
  if (res.statusCode !== 204) throw new Error(`login: ${res.statusCode}`);
  const cookie = res.headers['set-cookie'][0].split(';')[0];
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`, { headers: { Origin: `http://127.0.0.1:${port}`, Cookie: cookie } });
  const pending = new Map();
  let next = 1;
  const events = [];
  const sessions = new Map();
  ws.on('message', (data, isBinary) => {
    if (isBinary) return;
    const m = JSON.parse(data.toString());
    if (m.t === 'snapshot') for (const s of m.sessions) sessions.set(s.id, s);
    if (m.t === 'ev' && m.ev === 'session') sessions.set(m.session.id, m.session);
    if (m.t === 'ev' && m.ev === 'session_removed') sessions.delete(m.sid);
    if (m.t === 'res' && pending.has(m.id)) {
      const { resolve, reject } = pending.get(m.id);
      pending.delete(m.id);
      if (m.ok) resolve(m.data); else reject(new Error(`${m.error}`));
    } else if (m.t === 'ev') events.push(m);
  });
  await new Promise((resolve, reject) => { ws.on('open', resolve); ws.on('error', reject); });
  return {
    events,
    sessions,
    req(op, args = {}) {
      const id = next++;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        ws.send(JSON.stringify({ t: 'req', id, op, ...args }));
      });
    },
    close() { ws.terminate(); },
  };
}

export async function startServer({ proxy = false } = {}) {
  const home = os.homedir();
  const base = path.join(home, '.cache/claudio/web-e2e');
  const root = path.join(base, crypto.randomBytes(4).toString('hex'));
  for (const d of ['run', 'cfg/claudio', 'home', 'claude-config', 'xdg-cache', 'fake', 'work/a', 'work/b', 'work/repo']) {
    fs.mkdirSync(path.join(root, d), { recursive: true });
  }
  fs.chmodSync(path.join(root, 'run'), 0o700);
  fs.writeFileSync(path.join(root, 'cfg/claudio/config.toml'),
    '[claude]\nupdate_check = "off"\nremote_check = "off"\ninstall = "off"\n');
  fs.copyFileSync(path.join(here, 'fake-claude.sh'), path.join(root, 'fake-claude'));
  fs.chmodSync(path.join(root, 'fake-claude'), 0o755);
  const dirs = {
    a: fs.realpathSync(path.join(root, 'work/a')),
    b: fs.realpathSync(path.join(root, 'work/b')),
    repo: fs.realpathSync(path.join(root, 'work/repo')),
  };
  git(dirs.repo, 'init', '-q', '-b', 'main');
  fs.writeFileSync(path.join(dirs.repo, 'a.txt'), 'original\n');
  git(dirs.repo, 'add', 'a.txt');
  git(dirs.repo, 'commit', '-q', '-m', 'first');
  fs.writeFileSync(path.join(dirs.repo, 'a.txt'), 'original\nchanged\n');

  const fakeProxy = proxy ? await startFakeProxy() : null;
  const extra = fakeProxy ? { CLAUDIO_PROXY_URL: `${PROXY_TOKEN}@${fakeProxy.addr}` } : {};
  const env = isolatedEnv(root, extra);
  // The isolation is the point: never let a test reach the user's own daemon.
  if (!env.XDG_RUNTIME_DIR.startsWith(base)) throw new Error('runtime dir is not isolated');

  const port = await freePort();
  const log = fs.openSync(path.join(root, 'web.log'), 'a');
  const child = spawn(BIN, ['web', '--listen', `127.0.0.1:${port}`, '--no-open'], { env, stdio: ['ignore', 'pipe', log] });
  const url = await new Promise((resolve, reject) => {
    let buf = '';
    const t = setTimeout(() => reject(new Error(`claudio web printed no URL: ${buf}`)), 30_000);
    child.stdout.on('data', (d) => {
      buf += d;
      const m = /claudio web: (\S+)/.exec(buf);
      if (m) { clearTimeout(t); resolve(m[1]); }
    });
    child.on('exit', (c) => reject(new Error(`claudio web exited ${c}`)));
  });
  child.stdout.resume();
  const token = /#t=(.+)$/.exec(url)[1];
  const api = await connectApi(port, token);
  // The local daemon starts with the server: ops on `local` need it connected.
  for (const end = Date.now() + 30_000; ; await sleep(100)) {
    const { hosts } = await api.req('hosts.list');
    if (hosts.some((h) => h.name === 'local' && h.connected)) break;
    if (Date.now() > end) throw new Error('local host never connected');
  }

  async function waitFor(fn, what, ms = 15_000) {
    const end = Date.now() + ms;
    for (;;) {
      const v = fn();
      if (v) return v;
      if (Date.now() > end) throw new Error(`timed out: ${what}`);
      await sleep(50);
    }
  }

  return {
    url, token, port, root, dirs, claudeConfig: path.join(root, 'claude-config'),
    configToml: path.join(root, 'cfg/claudio/config.toml'),
    /** Run the saved hook command of the session started in `cwd`, JSON on stdin, as claude does. */
    async fireHook(cwd, event, payload = {}) {
      const file = path.join(root, 'fake/settings', `${mangle(cwd)}.json`);
      const settings = JSON.parse(await waitFor(() => { try { return fs.readFileSync(file, 'utf8'); } catch { return null; } }, `settings for ${cwd}`));
      const command = settings.hooks[event][0].hooks[0].command;
      const r = spawnSync('sh', ['-c', command], { input: JSON.stringify(payload), env });
      if (r.status !== 0) throw new Error(`hook ${event}: ${r.stderr}`);
    },
    /** The lines the fake claude of `cwd` read so far (a raw string; an empty line is "\n"). */
    inputLog(cwd) {
      try { return fs.readFileSync(path.join(root, 'fake/input', `${mangle(cwd)}.log`), 'utf8'); } catch { return ''; }
    },
    /** The env the fake claude of `cwd` dumped at launch (null until it did). */
    launchedEnv(cwd) {
      try { return fs.readFileSync(path.join(root, 'fake/env', `${mangle(cwd)}.txt`), 'utf8'); } catch { return null; }
    },
    waitFor,
    api: (op, args) => api.req(op, args),
    /** Spawn a claude session through the API; its sid. */
    async spawn(cwd, extraArgs = {}) {
      const r = await api.req('session.spawn', { host: 'local', cwd, rows: 30, cols: 100, proxy: { kind: 'direct' }, ...extraArgs });
      return r.sid;
    },
    /** Kill every session and forget what the fake claudes wrote, so a test starts clean. */
    async reset() {
      for (const sid of [...api.sessions.keys()]) await api.req('session.kill', { sid }).catch(() => {});
      await waitFor(() => api.sessions.size === 0, 'sessions gone', 10_000).catch(() => {});
      await api.req('layout.save', { layout: { grid: 1, panes: [null], focus: 0 } }).catch(() => {});
      for (const d of ['settings', 'input', 'env']) fs.rmSync(path.join(root, 'fake', d), { recursive: true, force: true });
    },
    async stop() {
      api.close();
      child.kill();
      await new Promise((r) => (child.exitCode !== null ? r() : child.once('exit', r)));
      // Stop the daemon of this root only (same isolated env); it is never the user's.
      spawnSync(BIN, ['daemon', 'stop'], { env, timeout: 15_000 });
      if (fakeProxy) await fakeProxy.stop();
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}
