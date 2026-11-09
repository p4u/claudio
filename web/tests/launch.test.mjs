import test from 'node:test';
import assert from 'node:assert/strict';
import { optionsByGroup, defaultOption, launchFlags, groupChoices, groupValue, flagsAllowed, selectFrom, labelFor, reviewLoop, optionValue, parseOption } from '../app/launch.js';

const decls = [
  { name: 'vocdoni', panels: [], launch_flags: [{ name: 'first-party', label: 'First-party session' }],
    launch_options: [
      { id: 'work', label: 'work', group: 'Proxy', default: true, detail: null },
      { id: 'home:x', label: 'home', group: 'Proxy', default: false, detail: 'first-party' },
    ] },
  { name: 'other', panels: [], launch_flags: [{ name: 'first-party', label: 'dup' }, { name: 'fast', label: 'Fast' }],
    launch_options: [{ id: 'eu', label: 'EU', group: 'Region', default: false, detail: null }] },
];

test('options are grouped by LaunchOption.group in the order listed', () => {
  const g = optionsByGroup(decls);
  assert.deepEqual(g.map((x) => x.group), ['Proxy', 'Region']);
  assert.deepEqual(g[0].options.map((o) => o.value), ['option:vocdoni:work', 'option:vocdoni:home:x']);
  assert.equal(g[0].options[1].detail, 'first-party');
  assert.deepEqual(optionsByGroup(null), []);
});

test('an option value round-trips, ids with a colon included', () => {
  assert.deepEqual(parseOption(optionValue('vocdoni', 'home:x')), { addon: 'vocdoni', option: 'home:x' });
  assert.equal(parseOption('default'), null);
  assert.equal(parseOption('option:vocdoni:'), null);
  assert.equal(parseOption('option::x'), null);
});

test('the default option labels the default entry, else direct', () => {
  assert.equal(defaultOption(decls).id, 'work');
  const [proxy] = optionsByGroup(decls);
  const c = groupChoices(proxy, decls);
  assert.deepEqual(c.map((x) => x.value), ['default', 'direct', 'option:vocdoni:work', 'option:vocdoni:home:x']);
  assert.equal(c[0].label, 'default (work)');
  const noDefault = [{ ...decls[0], launch_options: [{ ...decls[0].launch_options[0], default: false }] }];
  assert.equal(groupChoices(optionsByGroup(noDefault)[0], noDefault)[0].label, 'default (direct)');
  assert.equal(defaultOption([]), null);
});

test('a group select shows default for a pick of another group', () => {
  const [proxy, region] = optionsByGroup(decls);
  assert.equal(groupValue(proxy, 'option:vocdoni:work'), 'option:vocdoni:work');
  assert.equal(groupValue(region, 'option:vocdoni:work'), 'default');
  assert.equal(groupValue(region, 'direct'), 'direct');
});

test('labelled flags are listed once per name', () => {
  assert.deepEqual(launchFlags(decls), [{ name: 'first-party', label: 'First-party session' }, { name: 'fast', label: 'Fast' }]);
});

test('flags need an option in effect', () => {
  assert.equal(flagsAllowed('direct', decls), false);
  assert.equal(flagsAllowed('default', decls), true);
  assert.equal(flagsAllowed('default', []), false);
  assert.equal(flagsAllowed('option:other:eu', []), true);
});

test('selectFrom builds a LaunchSelect', () => {
  assert.deepEqual(selectFrom('default'), { pick: 'default', flags: [] });
  assert.deepEqual(selectFrom('direct', ['x']), { pick: 'direct', flags: ['x'] });
  assert.deepEqual(selectFrom('option:vocdoni:work', ['first-party', 'first-party']), { pick: 'option', addon: 'vocdoni', option: 'work', flags: ['first-party'] });
  assert.deepEqual(selectFrom('bogus'), { pick: 'default', flags: [] });
});

test('labelFor says unknown, direct or the option and its flags', () => {
  assert.equal(labelFor(null), 'unknown');
  assert.equal(labelFor({ known: false, choice: null }), 'unknown');
  assert.equal(labelFor({ known: true, choice: null }), 'direct');
  assert.equal(labelFor({ known: true, choice: { addon: 'vocdoni', option: 'work', flags: [] } }), 'work');
  assert.equal(labelFor({ known: true, choice: { addon: 'vocdoni', option: 'work', flags: ['first-party'] } }), 'work (first-party)');
});

test('reviewLoop: save spawns again, skip defers, cancel sends nothing more', async () => {
  const review = { addon: 'vocdoni', id: 'p1', revision: 'r1' };
  const sent = [];
  const replies = [{ review }, { review: { ...review, id: 'p2' } }, { sid: 's1' }];
  const spawn = async (defer) => { sent.push(defer); return replies.shift(); };
  const answers = ['saved', 'skip'];
  const r = await reviewLoop(spawn, async () => answers.shift());
  assert.deepEqual(r, { sid: 's1' });
  assert.deepEqual(sent, [[], [], [{ addon: 'vocdoni', id: 'p2' }]]);

  const sent2 = [];
  const r2 = await reviewLoop(async (d) => { sent2.push(d); return { review }; }, async () => 'cancel');
  assert.equal(r2, null);
  assert.equal(sent2.length, 1);

  await assert.rejects(reviewLoop(async () => ({ review }), async () => 'skip', 3), /review/);
  const r3 = await reviewLoop(async () => ({ sid: 'x' }), async () => assert.fail('no review asked'));
  assert.deepEqual(r3, { sid: 'x' });
});
