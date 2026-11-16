import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_LISTED, isPath, splitPath, useTarget, resolveTilde, abbreviateHome, startFolder, folderRows, useRow,
  firstHighlight, complete, backspaceUp, recentRows, sequencer, listNote, listingOf,
} from '../app/dirnav.js';

const dir = (name) => ({ name, dir: true, hidden: name.startsWith('.') });
const file = (name) => ({ name, dir: false, hidden: false });

test('a typed path splits into the listed parent and the fragment', () => {
  assert.deepEqual(splitPath('~/src/cla'), { parent: '~/src/', fragment: 'cla' });
  assert.deepEqual(splitPath('~/src/'), { parent: '~/src/', fragment: '' });
  assert.deepEqual(splitPath('~'), { parent: '~/', fragment: '' });
  assert.deepEqual(splitPath('/'), { parent: '/', fragment: '' });
  assert.deepEqual(splitPath('/srv'), { parent: '/', fragment: 'srv' });
  assert.ok(isPath('~/x') && isPath('/x') && isPath(' /x'));
  assert.ok(!isPath('claudio') && !isPath(''));
});

test('the use row names the typed folder, never its parent', () => {
  assert.equal(useTarget('~/src/'), '~/src');
  assert.equal(useTarget('~/src/cla'), '~/src/cla');
  assert.equal(useTarget('/'), '/');
  assert.equal(useTarget('~/'), '~');
  const listing = { parent: '~/src/', entries: [dir('claudio'), file('notes')], truncated: false, error: null };
  assert.deepEqual(useRow('~/src/claudio', listing), { target: '~/src/claudio', notFound: false });
  assert.deepEqual(useRow('~/src/cla', listing), { target: '~/src/cla', notFound: true });
  assert.deepEqual(useRow('~/src/notes', listing), { target: '~/src/notes', notFound: true }, 'a file is not a folder');
  assert.equal(useRow('~/src/', listing).notFound, false, 'an empty fragment is the listed folder');
});

test('not found needs a whole listing of that parent', () => {
  const whole = { parent: '~/', entries: [dir('a')], truncated: false, error: null };
  assert.equal(useRow('~/missing', whole).notFound, true);
  assert.equal(useRow('~/missing', null).notFound, false, 'not arrived yet');
  assert.equal(useRow('~/missing', { ...whole, entries: null }).notFound, false, 'still reading');
  assert.equal(useRow('~/missing', { ...whole, parent: '~/x/' }).notFound, false, 'another parent');
  assert.equal(useRow('~/missing', { ...whole, truncated: true }).notFound, false, 'cut by the daemon');
  assert.equal(useRow('~/missing', { ...whole, error: 'cannot list' }).notFound, false);
  const full = Array.from({ length: MAX_LISTED }, (_, i) => dir(`d${i}`));
  assert.equal(useRow('~/missing', { ...whole, entries: full }).notFound, false, 'an old daemon cut at 5000 silently');
});

test('folder rows: folders only, prefix-filtered and sorted case-insensitively', () => {
  // An old daemon's listing: directory order, files mixed in.
  const entries = [file('b.txt'), dir('zed'), file('A.md'), dir('Alpha'), dir('beta'), dir('.cfg'), dir('alps')];
  assert.deepEqual(folderRows(entries, ''), ['Alpha', 'alps', 'beta', 'zed']);
  assert.deepEqual(folderRows(entries, 'AL'), ['Alpha', 'alps']);
  assert.deepEqual(folderRows(entries, 'q'), []);
});

test('hidden folders: off by default, on with the toggle or a dot fragment', () => {
  const entries = [dir('.cfg'), dir('src'), dir('.cache')];
  assert.deepEqual(folderRows(entries, ''), ['src']);
  assert.deepEqual(folderRows(entries, '', true), ['.cache', '.cfg', 'src']);
  assert.deepEqual(folderRows(entries, '.'), ['.cache', '.cfg']);
  assert.deepEqual(folderRows(entries, '.cf'), ['.cfg']);
});

test('the highlight: use with no fragment or no match, else the first folder', () => {
  assert.equal(firstHighlight('', ['a']), 0);
  assert.equal(firstHighlight('a', ['a', 'ab']), 1);
  assert.equal(firstHighlight('q', []), 0);
});

test('completing a folder and going up with backspace', () => {
  assert.equal(complete('~/src/', 'claudio'), '~/src/claudio/');
  assert.equal(backspaceUp('~/a/b/', '/home/u'), '~/a/');
  assert.equal(backspaceUp('~/a/', '/home/u'), '~/');
  assert.equal(backspaceUp('~/', '/home/u'), '/home/');
  assert.equal(backspaceUp('~/', null), null, 'home unknown: an ordinary backspace');
  assert.equal(backspaceUp('/srv/app/', null), '/srv/');
  assert.equal(backspaceUp('/srv/', null), '/');
  assert.equal(backspaceUp('/', null), '/');
  assert.equal(backspaceUp('~/a/b', '/home/u'), null, 'no trailing slash: one character');
});

test('~ is resolved with the host home before anything is sent', () => {
  assert.equal(resolveTilde('~', '/home/u'), '/home/u');
  assert.equal(resolveTilde('~/src', '/home/u/'), '/home/u/src');
  assert.equal(resolveTilde('~/src', '/'), '/src');
  assert.equal(resolveTilde('~/src', null), null);
  assert.equal(resolveTilde('/srv', null), '/srv');
  assert.equal(resolveTilde('~other/x', '/home/u'), '~other/x');
  assert.deepEqual(startFolder('~/src/', '/home/u'), { cwd: '/home/u/src' });
  assert.deepEqual(startFolder('/srv/app', null), { cwd: '/srv/app' });
  assert.deepEqual(startFolder('', null), { why: null });
  assert.deepEqual(startFolder('claudio', '/home/u'), { why: 'Type a path (it starts with / or ~)' });
  assert.deepEqual(startFolder('~/x', null), { why: 'Reading the home folder…' });
  assert.deepEqual(startFolder('~/x', null, 'timed out'), { why: 'cannot read the home folder: timed out' });
  assert.ok(startFolder('~other', '/home/u').why.includes('~user'));
  assert.equal(abbreviateHome('/home/u/src', '/home/u'), '~/src');
  assert.equal(abbreviateHome('/home/uu', '/home/u'), '/home/uu');
  assert.equal(abbreviateHome('/srv', null), '/srv');
});

test('recent folders filter by substring, case-insensitively', () => {
  const dirs = [{ path: '/home/dev/Claudio' }, { path: '/home/dev/api' }, { path: '/srv/infra' }];
  assert.equal(recentRows(dirs, '').length, 3);
  assert.deepEqual(recentRows(dirs, 'claud').map((d) => d.path), ['/home/dev/Claudio']);
  assert.deepEqual(recentRows(dirs, 'DEV').map((d) => d.path), ['/home/dev/Claudio', '/home/dev/api']);
  assert.deepEqual(recentRows(dirs, 'dvapi'), [], 'substring, not fuzzy');
});

test('a listing reply for an earlier request is ignored', () => {
  const seq = sequencer();
  const first = seq.next();
  const second = seq.next();
  assert.equal(seq.fresh(first), false);
  assert.equal(seq.fresh(second), true);
  seq.drop(); // the host changed
  assert.equal(seq.fresh(second), false);
});

test('the note under the field: the error, the cut, or nothing', () => {
  const ok = { parent: '~/', entries: [], truncated: false, error: null };
  assert.equal(listNote(null), null);
  assert.equal(listNote(ok), null);
  assert.equal(listNote({ ...ok, error: 'cannot list ~/f: Not a directory' }), 'cannot list ~/f: Not a directory');
  assert.equal(listNote({ ...ok, truncated: true }), "list cut: type the folder's full name");
  assert.equal(listNote({ ...ok, truncated: true, error: 'boom' }), 'boom');
});

test('not found compares names without case (a case-insensitive filesystem)', () => {
  const whole = { parent: '~/', entries: [dir('Documents')], truncated: false, error: null };
  assert.equal(useRow('~/documents', whole).notFound, false);
  assert.equal(useRow('~/docs', whole).notFound, true);
});

test('a cut listing is cut for folders only when its last entry is a folder', () => {
  const cutFolders = listingOf('~/', { entries: [dir('a'), dir('b')], truncated: true });
  assert.equal(cutFolders.truncated, true);
  assert.equal(listNote(cutFolders), "list cut: type the folder's full name");
  assert.equal(useRow('~/zz', cutFolders).notFound, false);
  const cutFiles = listingOf('~/', { entries: [dir('a'), file('x'), file('y')], truncated: true });
  assert.equal(cutFiles.truncated, false);
  assert.equal(listNote(cutFiles), null, 'only files were cut');
  assert.equal(useRow('~/zz', cutFiles).notFound, true);
  assert.equal(listingOf('~/', { entries: [], truncated: true }).truncated, true, 'nothing listed: unknown');
  assert.equal(listingOf('~/', { entries: [dir('a')], truncated: false }).truncated, false);
});
