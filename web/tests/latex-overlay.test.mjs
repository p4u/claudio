import test from 'node:test';
import assert from 'node:assert/strict';
import { mdUnescape, normalize, normalizeLoose, withoutHtml, formulaKeys, matchFormula, findSpans } from '../app/latex-overlay.js';

// A screen row of `s`: one glyph per character at its starting column; CJK takes two columns.
const wide = (ch) => /[ᄀ-ᅟ⺀-꓏가-힣＀-｠]/.test(ch);
function row(s) {
  const glyphs = [];
  let col = 0;
  for (const ch of s) { glyphs.push({ col, ch }); col += wide(ch) ? 2 : 1; }
  return { glyphs, width: col };
}
const rows = (...ss) => ss.map(row);
const spans = (...ss) => findSpans(rows(...ss));

test('keys undo what markdown does on screen', () => {
  const k = formulaKeys('\\{a_i\\} \\cup \\{b_i\\} \\\\ x^2\\,dx');
  assert.equal(k.strict, '{a_i}\\cup{b_i}\\x^2,dx');
  assert.equal(k.loose, '{ai}\\cup{bi}\\x^2,dx');
  const h = formulaKeys('x <b> y </b> < z');
  assert.equal(h.html, 'xy<z');
  assert.equal(h.loose, 'x<b>y</b><z');
});

test('unescape, normalize and html helpers', () => {
  assert.equal(mdUnescape('a\\{b\\q\\'), 'a{b\\q\\');
  assert.equal(normalize(' a\tb c\n'), 'abc');
  assert.equal(normalizeLoose('a*b_c~d`e f'), 'abcde' + 'f');
  assert.equal(withoutHtml('a<i>b</i>c<3 d>'), 'abc<3 d>');
  assert.equal(withoutHtml('a <b <c> d'), 'a <b  d');
});

test('one tag on a row', () => {
  const s = spans('hi <latex>E = mc^2</latex> there');
  assert.deepEqual(s.map(({ row, col, endRow, endCol, text }) => ({ row, col, endRow, endCol, text })),
    [{ row: 0, col: 3, endRow: 0, endCol: 26, text: 'E = mc^2' }]);
});

test('two tags on a row', () => {
  const s = spans('<latex>a</latex> and <latex>b</latex>');
  assert.deepEqual(s.map((x) => [x.col, x.endCol, x.text]), [[0, 16, 'a'], [21, 37, 'b']]);
});

test('a non-latex tag name is ignored, a named lens tag is found', () => {
  assert.deepEqual(spans('<other>x</other>'), []);
  assert.equal(findSpans(rows('<other>x</other>'), 'other').length, 1);
});

test('a tag continued on an indented row is joined with single spaces', () => {
  const s = spans('  <latex>a +', '    b +   ', '    c</latex> tail', 'next');
  assert.equal(s.length, 1);
  assert.equal(s[0].text, 'a + b + c');
  assert.deepEqual([s[0].row, s[0].col, s[0].endRow, s[0].endCol], [0, 2, 2, 13]);
  assert.deepEqual(s[0].cont, [{ row: 1, col: 4, endCol: 10 }, { row: 2, col: 4, endCol: 13 }]);
});

test('another opening tag on a continuation row stops the span', () => {
  const s = spans('<latex>a +', '  <latex>b</latex>', 'c</latex>');
  assert.deepEqual(s.map((x) => x.text), ['b']);
});

test('a span with other tags on its first row is not joined', () => {
  assert.deepEqual(spans('<latex>a</latex> <latex>b +', ' c</latex>').map((x) => x.text), ['a']);
});

test('unclosed tags and too-long spans yield nothing', () => {
  assert.deepEqual(spans('<latex>a', 'b', 'c'), []);
  const long = ['<latex>x', ...Array(9).fill('y'), '</latex>'];
  assert.deepEqual(spans(...long), []);
  const ok = ['<latex>x', ...Array(7).fill('y'), '</latex>'];
  assert.equal(spans(...ok).length, 1);
});

test('a wide character before a tag keeps columns right', () => {
  const s = spans('日本<latex>x</latex>');
  assert.deepEqual([s[0].col, s[0].endCol], [4, 20]);
});

test('matches after markdown ate escapes, emphasis or html', () => {
  const f = ['\\{a_i\\} \\cup \\{b_i\\}', 'x_1 * y_2', 'p <b> q </b> r'];
  assert.equal(matchFormula(f, '{a_i} \\cup {b_i}'), f[0]);
  assert.equal(matchFormula(f, 'x1 y2'), f[1]);
  assert.equal(matchFormula(f, 'x_1*y_2'), f[1]);
  assert.equal(matchFormula(f, 'p q r'), f[2]);
});

test('unmatched text yields null; the newest formula wins', () => {
  assert.equal(matchFormula(['a+b'], 'a+c'), null);
  assert.equal(matchFormula([], 'a'), null);
  assert.equal(matchFormula(['a + b', 'a+b', 'c'], 'a  +b'), 'a+b');
  // strict beats loose even when an older formula is strict
  assert.equal(matchFormula(['a_b', 'ab'], 'a_b'), 'a_b');
});

test('formulasOf prefers the latex lens items, else the latex event list', async () => {
  const { formulasOf } = await import('../app/latex-overlay.js');
  assert.deepEqual(formulasOf(undefined), []);
  assert.deepEqual(formulasOf({ items: [{ tag: 'latex', text: 'a' }, { tag: 'x', text: 'b' }], formulas: ['z'] }), ['a']);
  assert.deepEqual(formulasOf({ items: [], formulas: ['z'] }), ['z']);
});
