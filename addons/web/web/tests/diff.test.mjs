import test from 'node:test';
import assert from 'node:assert/strict';
import { parsePatch, sideBySide, groupWorktree } from '../app/diff.js';

const PATCH = [
  'diff --git a/src/a.rs b/src/a.rs',
  'index 1111111..2222222 100644',
  '--- a/src/a.rs',
  '+++ b/src/a.rs',
  '@@ -3,4 +3,5 @@ fn main() {',
  ' keep one',
  '-old two',
  '+new two',
  '+new three',
  ' keep four',
  '\\ No newline at end of file',
  '@@ -20,2 +21,2 @@',
  '-x',
  '+y',
  ' z',
  '',
].join('\n');

test('a unified patch parses into hunks with line numbers', () => {
  const { files } = parsePatch(PATCH);
  assert.equal(files.length, 1);
  const f = files[0];
  assert.equal(f.oldPath, 'src/a.rs');
  assert.equal(f.newPath, 'src/a.rs');
  assert.equal(f.binary, false);
  assert.equal(f.hunks.length, 2);
  const h = f.hunks[0];
  assert.equal(h.header, '@@ -3,4 +3,5 @@ fn main() {');
  assert.deepEqual(h.lines.map((l) => l.kind), ['ctx', 'del', 'add', 'add', 'ctx', 'meta']);
  assert.deepEqual(h.lines.map((l) => [l.old, l.new]), [[3, 3], [4, null], [null, 4], [null, 5], [5, 6], [null, null]]);
  assert.equal(h.lines[1].text, 'old two');
  assert.equal(h.lines[5].text, '\\ No newline at end of file');
  assert.deepEqual(f.hunks[1].lines.map((l) => [l.old, l.new]), [[20, null], [null, 21], [21, 22]]);
});

test('side-by-side pairs removals with additions', () => {
  const h = parsePatch(PATCH).files[0].hunks[0];
  const rows = sideBySide(h);
  assert.equal(rows.length, 5);
  assert.equal(rows[0].left.text, 'keep one');
  assert.equal(rows[0].right.text, 'keep one');
  assert.equal(rows[1].left.text, 'old two');
  assert.equal(rows[1].right.text, 'new two');
  assert.equal(rows[2].left, null);
  assert.equal(rows[2].right.text, 'new three');
  const only = sideBySide({ header: '', lines: [{ kind: 'del', text: 'a', old: 1, new: null }, { kind: 'del', text: 'b', old: 2, new: null }, { kind: 'add', text: 'c', old: null, new: 1 }] });
  assert.deepEqual(only.map((r) => [r.left?.text ?? null, r.right?.text ?? null]), [['a', 'c'], ['b', null]]);
});

test('binary files and renames are recognised', () => {
  const text = [
    'diff --git a/img/a.png b/img/b.png',
    'similarity index 100%',
    'rename from img/a.png',
    'rename to img/b.png',
    'diff --git a/logo.bin b/logo.bin',
    'index 1..2 100644',
    'Binary files a/logo.bin and b/logo.bin differ',
    'diff --git a/new.txt b/new.txt',
    'new file mode 100644',
    '--- /dev/null',
    '+++ b/new.txt',
    '@@ -0,0 +1 @@',
    '+hello',
  ].join('\n');
  const { files } = parsePatch(text);
  assert.equal(files.length, 3);
  assert.equal(files[0].oldPath, 'img/a.png');
  assert.equal(files[0].newPath, 'img/b.png');
  assert.equal(files[0].binary, false);
  assert.equal(files[0].hunks.length, 0);
  assert.equal(files[1].binary, true);
  assert.equal(files[2].oldPath, null);
  assert.equal(files[2].newPath, 'new.txt');
  assert.deepEqual(files[2].hunks[0].lines, [{ kind: 'add', text: 'hello', old: null, new: 1 }]);
  assert.deepEqual(parsePatch('').files, []);
});

test('worktree files group into conflicts, staged, unstaged and untracked', () => {
  const f = (path, index, worktree) => ({ path, old_path: null, index, worktree, added: 1, removed: 0 });
  const g = groupWorktree([f('a', 'M', '.'), f('b', '.', 'M'), f('c', 'M', 'M'), f('d', '?', '?'), f('e', 'U', 'U'), f('f', 'A', 'D')]);
  assert.deepEqual(g.conflicts.map((x) => x.path), ['e']);
  assert.deepEqual(g.staged.map((x) => x.path), ['a', 'c', 'f']);
  assert.deepEqual(g.unstaged.map((x) => x.path), ['b', 'c', 'f']);
  assert.deepEqual(g.untracked.map((x) => x.path), ['d']);
  assert.deepEqual(groupWorktree([]), { conflicts: [], staged: [], unstaged: [], untracked: [] });
});
