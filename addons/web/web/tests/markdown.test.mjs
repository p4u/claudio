import test from 'node:test';
import assert from 'node:assert/strict';
import { extractLatex, answerFormulas, placeholderRe, fenceLang, MARKDOWN_PURIFY, SVG_PURIFY, KATEX_OPTIONS, MERMAID_CONFIG } from '../app/render/markdown.js';

const NONCE = 'abc123';

test('extractLatex replaces tags outside code with this render\'s placeholders', () => {
  const { text, formulas } = extractLatex('Energy is <latex>E=mc^2</latex> and <latex>a+b</latex>.', NONCE);
  assert.equal(formulas.length, 2);
  assert.deepEqual(formulas.map((f) => f.tex), ['E=mc^2', 'a+b']);
  assert.ok(!text.includes('<latex>'));
  const hits = [...text.matchAll(placeholderRe(NONCE))];
  assert.deepEqual(hits.map((m) => Number(m[1])), [0, 1]);
});

test('latex inside fenced and inline code is left alone', () => {
  const md = 'Use `<latex>x</latex>` here.\n\n```html\n<latex>y</latex>\n```\n\n~~~\n<latex>z</latex>\n~~~\n\nreal <latex>w</latex>';
  const { text, formulas } = extractLatex(md, NONCE);
  assert.deepEqual(formulas.map((f) => f.tex), ['w']);
  assert.ok(text.includes('`<latex>x</latex>`'));
  assert.ok(text.includes('<latex>y</latex>'));
  assert.ok(text.includes('<latex>z</latex>'));
});

test('a tag alone on its line is display math, inside a sentence inline', () => {
  const md = 'Before\n\n<latex>\\int_0^1 x\\,dx</latex>\n\nthen <latex>x</latex> inline\n\n  <latex>y</latex>  \n';
  const { formulas } = extractLatex(md, NONCE);
  assert.deepEqual(formulas.map((f) => [f.tex, f.display]), [['\\int_0^1 x\\,dx', true], ['x', false], ['y', true]]);
});

test('model text cannot forge a placeholder', () => {
  const { text, formulas } = extractLatex('x:0 and y:0', 'y');
  assert.equal(formulas.length, 0);
  assert.equal([...text.matchAll(placeholderRe('y'))].length, 0, 'private-use characters in model text are dropped');
  assert.ok(text.includes('x:0'));
});

test('KATEX_OPTIONS keep trust off and cap size and expansion', () => {
  assert.equal(KATEX_OPTIONS.trust, false);
  assert.equal(KATEX_OPTIONS.maxSize, 20);
  assert.equal(KATEX_OPTIONS.maxExpand, 1000);
  assert.equal(KATEX_OPTIONS.throwOnError, true);
  assert.equal(KATEX_OPTIONS.strict, 'ignore');
});

test('MERMAID_CONFIG is strict with SVG labels', () => {
  assert.equal(MERMAID_CONFIG.securityLevel, 'strict');
  assert.equal(MERMAID_CONFIG.htmlLabels, false);
  assert.equal(MERMAID_CONFIG.flowchart.htmlLabels, false);
  assert.equal(MERMAID_CONFIG.startOnLoad, false);
  assert.equal(SVG_PURIFY.USE_PROFILES.svg, true);
});

test('MARKDOWN_PURIFY forbids script, style, iframe and style attributes', () => {
  for (const t of ['script', 'style', 'iframe', 'form', 'input', 'object', 'embed']) assert.ok(MARKDOWN_PURIFY.FORBID_TAGS.includes(t), t);
  assert.ok(MARKDOWN_PURIFY.FORBID_ATTR.includes('style'));
  assert.equal(MARKDOWN_PURIFY.ALLOW_DATA_ATTR, false);
});

test('fenceLang reads the language class', () => {
  assert.equal(fenceLang('language-mermaid'), 'mermaid');
  assert.equal(fenceLang('hljs language-rust'), 'rust');
  assert.equal(fenceLang(''), null);
  assert.equal(fenceLang(undefined), null);
});

test('stripEventHandlers drops on*= attributes from raw tags and leaves text alone', async () => {
  const { stripEventHandlers } = await import('../app/render/markdown.js');
  assert.equal(stripEventHandlers('<img src=x onerror=alert(1)>'), '<img src=x>');
  assert.equal(stripEventHandlers('<a href="u" onclick="a(1)" onmouseover=\'b\'>t</a>'), '<a href="u">t</a>');
  assert.equal(stripEventHandlers('<svg/onload=alert(1)>'), '<svg>');
  assert.equal(stripEventHandlers('plain text with onclick=foo inside'), 'plain text with onclick=foo inside');
});

test('answerFormulas lists the distinct formulas of the answers, skipping code', () => {
  const answers = [
    { text: 'See <latex>a+b</latex> and <latex>c</latex>.\n```\n<latex>nope</latex>\n```' },
    { text: 'Again <latex>a+b</latex>, plus\n\n<latex>\\int x</latex>' },
    { text: 'none here' },
  ];
  assert.deepEqual(answerFormulas(answers), ['a+b', 'c', '\\int x']);
  assert.deepEqual(answerFormulas([]), []);
  assert.deepEqual(answerFormulas(undefined), []);
});
