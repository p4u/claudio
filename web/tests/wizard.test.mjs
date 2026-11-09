import test from 'node:test';
import assert from 'node:assert/strict';
import { initialWizard, spawnRequest, filterDirs } from '../app/wizard.js';

const decls = [{ name: 'vocdoni', panels: [], launch_flags: [{ name: 'first-party', label: 'First-party session' }],
  launch_options: [{ id: 'work', label: 'work', group: 'Proxy', default: true, detail: null }, { id: 'personal', label: 'personal', group: 'Proxy', default: false, detail: null }] }];

test('the wizard starts on the default launch, no flags', () => {
  const w = initialWizard({ hosts: ['local'], decls, recent: [], focusedHost: 'local' });
  assert.equal(w.launch, 'default');
  assert.deepEqual(w.flags, []);
  assert.deepEqual(spawnRequest({ ...w, cwd: '/a' }, { rows: 24, cols: 80 }).args.launch, { pick: 'default', flags: [] });
});

test('spawn request for claude carries launch, defer, resume, prompt, name and size', () => {
  const w = { ...initialWizard({ hosts: ['local', 'z6'], decls, recent: [], focusedHost: 'z6' }), cwd: '/srv/app', resume: 'conv-1', name: ' api ', prompt: 'hello', launch: 'option:vocdoni:work', flags: ['first-party'] };
  const r = spawnRequest(w, { rows: 30, cols: 100 }, [{ addon: 'vocdoni', id: 'p1', revision: 'x' }]);
  assert.equal(r.op, 'session.spawn');
  assert.deepEqual(r.args, { host: 'z6', cwd: '/srv/app', rows: 30, cols: 100, launch: { pick: 'option', addon: 'vocdoni', option: 'work', flags: ['first-party'] }, defer: [{ addon: 'vocdoni', id: 'p1' }], resume: 'conv-1', name: 'api', prompt: 'hello' });
  const bare = spawnRequest({ ...w, resume: null, name: '', prompt: '', flags: [] }, { rows: 24, cols: 80 });
  assert.deepEqual(Object.keys(bare.args).sort(), ['cols', 'cwd', 'host', 'launch', 'rows']);
  const direct = spawnRequest({ ...w, launch: 'direct' }, { rows: 24, cols: 80 });
  assert.deepEqual(direct.args.launch, { pick: 'direct', flags: [] }, 'flags never go out without an option in effect');
  const noDefault = spawnRequest({ ...w, decls: [], launch: 'default' }, { rows: 24, cols: 80 });
  assert.deepEqual(noDefault.args.launch.flags, []);
});

test('a terminal spawns a shell without launch, resume or prompt', () => {
  const w = { ...initialWizard({ hosts: ['local'], decls, recent: [], focusedHost: 'local' }), kind: 'shell', cwd: '/tmp/x', resume: 'c', prompt: 'p', launch: 'direct', name: 'sh' };
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
