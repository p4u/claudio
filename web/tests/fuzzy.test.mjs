import test from 'node:test';
import assert from 'node:assert/strict';
import { score, filter } from '../app/fuzzy.js';

test('subsequences match and contiguous runs rank higher', () => {
  assert.ok(score('abc', 'a-b-c') > 0);
  assert.ok(score('abc', 'xxabcxx') > score('abc', 'axbxc'));
  assert.ok(score('kill', 'Kill session') > score('kill', 'keep in lost list'));
  assert.ok(score('', 'anything') !== null);
  assert.equal(score('ABC', 'abc') , score('abc', 'ABC'));
});

test('no match is null', () => {
  assert.equal(score('abc', 'acb'), null);
  assert.equal(score('z', 'abc'), null);
});

test('filter keeps input order on ties', () => {
  const items = [{ t: 'beta one' }, { t: 'alpha one' }, { t: 'gamma' }, { t: 'one' }];
  assert.deepEqual(filter(items, '', (x) => x.t).map((x) => x.t), items.map((x) => x.t));
  const r = filter(items, 'one', (x) => x.t).map((x) => x.t);
  assert.deepEqual(r.slice(0, 1), ['one']);
  assert.deepEqual(r.slice(1), ['beta one', 'alpha one']);
  assert.equal(r.includes('gamma'), false);
});
