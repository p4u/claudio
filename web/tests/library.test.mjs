import test from 'node:test';
import assert from 'node:assert/strict';
import { slug, firstLine, filterItems, nameFromText, hasName } from '../app/library.js';

test('slug follows the daemon rules', () => {
  assert.equal(slug('Release Notes!'), 'release-notes');
  assert.equal(slug('  --fix   the_tests\n now'), 'fix-the-tests-now');
  assert.equal(slug('Résumé ok'), 'r-sum-ok');
  assert.equal(slug('¿?'), 'snippet');
  assert.equal(slug(''), 'snippet');
  assert.equal(slug('x'.repeat(50)), 'x'.repeat(40));
  assert.equal(slug('one two three four five six seven eight nine ten'), 'one-two-three-four-five-six-seven-eight');
  assert.equal(slug('../../etc/passwd'), 'etc-passwd');
});

test('firstLine is the first non-blank line, trimmed and cut', () => {
  assert.equal(firstLine('\n\n  hello there  \nsecond'), 'hello there');
  assert.equal(firstLine(''), '');
  assert.equal(firstLine('x'.repeat(200), 20), `${'x'.repeat(19)}…`);
});

test('nameFromText pre-fills a name from the first line', () => {
  assert.equal(nameFromText('\n  Write the release notes for v2.\nmore'), 'Write the release notes for v2');
  assert.equal(nameFromText('x'.repeat(100)).length <= 40, true);
  assert.equal(nameFromText(''), '');
});

test('filterItems matches name and text, case-insensitively', () => {
  const items = [{ name: 'release-notes', text: 'Write notes' }, { name: 'fix', text: 'Fix the TESTS' }];
  assert.deepEqual(filterItems(items, 'NOTES').map((i) => i.name), ['release-notes']);
  assert.deepEqual(filterItems(items, 'tests').map((i) => i.name), ['fix']);
  assert.equal(filterItems(items, '  ').length, 2);
  assert.equal(filterItems(null, 'x').length, 0);
});

test('hasName finds the snippet a name would replace', () => {
  const items = [{ name: 'release-notes', text: 'a' }];
  assert.equal(hasName(items, 'Release Notes')?.name, 'release-notes');
  assert.equal(hasName(items, 'other'), null);
});
