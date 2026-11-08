import test from 'node:test';
import assert from 'node:assert/strict';
import { initialWizard, proxyOptions, proxyArg, firstPartyAllowed, spawnRequest, filterDirs } from '../app/wizard.js';

const profiles = { names: ['work', 'personal'], default: 'work' };

test('proxy options pre-select and label the [proxy] default', () => {
  const o = proxyOptions(profiles);
  assert.deepEqual(o.map((x) => x.value), ['default', 'direct', 'profile:work', 'profile:personal']);
  assert.equal(o[0].label, 'default (work)');
  assert.equal(proxyOptions({ names: [], default: null })[0].label, 'default (direct)');
  assert.equal(initialWizard({ hosts: ['local'], profiles, recent: [], focusedHost: 'local' }).proxy, 'default');
  assert.deepEqual(proxyArg('default'), { kind: 'default' });
  assert.deepEqual(proxyArg('direct'), { kind: 'direct' });
  assert.deepEqual(proxyArg('profile:work'), { kind: 'profile', name: 'work' });
});

test('first-party only with a proxy', () => {
  assert.equal(firstPartyAllowed('direct', profiles), false);
  assert.equal(firstPartyAllowed('profile:work', profiles), true);
  assert.equal(firstPartyAllowed('default', profiles), true);
  assert.equal(firstPartyAllowed('default', { names: [], default: null }), false);
});

test('spawn request for claude carries resume, prompt, name and size', () => {
  const w = { ...initialWizard({ hosts: ['local', 'z6'], profiles, recent: [], focusedHost: 'z6' }), cwd: '/srv/app', resume: 'conv-1', name: ' api ', prompt: 'hello', proxy: 'profile:work', firstParty: true, skip: ['p1'] };
  const r = spawnRequest(w, { rows: 30, cols: 100 });
  assert.equal(r.op, 'session.spawn');
  assert.deepEqual(r.args, { host: 'z6', cwd: '/srv/app', rows: 30, cols: 100, proxy: { kind: 'profile', name: 'work' }, first_party: true, resume: 'conv-1', name: 'api', prompt: 'hello', skip_plugins: ['p1'] });
  const bare = spawnRequest({ ...w, resume: null, name: '', prompt: '', firstParty: false, skip: [] }, { rows: 24, cols: 80 });
  assert.deepEqual(Object.keys(bare.args).sort(), ['cols', 'cwd', 'host', 'proxy', 'rows']);
  const direct = spawnRequest({ ...w, proxy: 'direct' }, { rows: 24, cols: 80 });
  assert.equal(direct.args.first_party, undefined, 'first-party never goes out without a proxy');
});

test('a terminal spawns a shell without proxy, resume or prompt', () => {
  const w = { ...initialWizard({ hosts: ['local'], profiles, recent: [], focusedHost: 'local' }), kind: 'shell', cwd: '/tmp/x', resume: 'c', prompt: 'p', proxy: 'direct', name: 'sh' };
  const r = spawnRequest(w, { rows: 24, cols: 80 });
  assert.equal(r.op, 'session.shell');
  assert.deepEqual(r.args, { host: 'local', cwd: '/tmp/x', rows: 24, cols: 80, name: 'sh' });
});

test('typed paths filter the recent dirs fuzzily', () => {
  const dirs = [{ path: '/home/dev/claudio' }, { path: '/home/dev/api' }, { path: '/srv/infra' }];
  assert.deepEqual(filterDirs(dirs, '').map((d) => d.path), dirs.map((d) => d.path));
  assert.deepEqual(filterDirs(dirs, 'dvapi').map((d) => d.path), ['/home/dev/api']);
  const typed = filterDirs(dirs, '/opt/new');
  assert.equal(typed[0].path, '/opt/new');
  assert.equal(typed[0].typed, true);
  assert.equal(filterDirs(dirs, '/srv/infra')[0].path, '/srv/infra');
  assert.equal(filterDirs(dirs, '/srv/infra').length, 1);
});
