// Renders model markdown into a container. The only place model text becomes DOM:
//   marked -> DOMPurify(MARKDOWN_PURIFY) -> container; formulas through KaTeX (trust off) and DOMPurify;
//   mermaid SVG and highlight.js output through DOMPurify; links and images are checked afterwards.
import { Marked } from 'marked';
import DOMPurify from 'dompurify';
import { extractLatex, placeholderRe, fenceLang, stripEventHandlers, MARKDOWN_PURIFY, SVG_PURIFY, KATEX_OPTIONS } from './markdown.js';
import { getKatex } from './katex.js';
import { renderDiagram } from './mermaid.js';
import { highlight } from './highlight.js';

const marked = new Marked({ gfm: true, renderer: { html: ({ text }) => stripEventHandlers(text) } });

const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

const nonceOf = () => [...crypto.getRandomValues(new Uint8Array(12))].map((b) => b.toString(16).padStart(2, '0')).join('');
// Images are replaced inside DOMPurify's inert tree, so the browser never starts to fetch one.
let inMarkdown = false;
DOMPurify.addHook('uponSanitizeElement', (node) => {
  if (!inMarkdown || node.nodeName !== 'IMG') return;
  const alt = node.getAttribute('alt') || node.getAttribute('src') || 'image';
  const note = node.ownerDocument.createElement('span');
  note.className = 'img-note';
  note.textContent = `[image: ${alt.slice(0, 80)}]`;
  node.parentNode?.replaceChild(note, node);
});

const SAFE_HREF = /^(https?:|mailto:|#)/i;

// -> an element holding one formula; an error leaves the source as text in a <code>.
export async function formulaNode(tex, display) {
  try {
    const katex = await getKatex();
    const out = katex.renderToString(tex, { ...KATEX_OPTIONS, displayMode: display });
    const box = el(display ? 'div' : 'span', display ? 'math-display' : 'math-inline');
    box.innerHTML = DOMPurify.sanitize(out); // default profile: html, svg and MathML, with style attributes KaTeX needs
    return box;
  } catch {
    const c = el('code', 'math-error', tex);
    c.title = 'This formula could not be typeset';
    return c;
  }
}

async function fillFormulas(container, formulas, nonce) {
  const re = placeholderRe(nonce);
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const nodes = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    re.lastIndex = 0;
    if (re.test(n.nodeValue)) nodes.push(n);
  }
  for (const node of nodes) {
    const text = node.nodeValue;
    const parts = [];
    let at = 0;
    re.lastIndex = 0;
    for (const m of text.matchAll(placeholderRe(nonce))) {
      const f = formulas[Number(m[1])];
      parts.push(text.slice(at, m.index));
      parts.push(f ? await formulaNode(f.tex, f.display) : '');
      at = m.index + m[0].length;
    }
    parts.push(text.slice(at));
    const p = node.parentElement;
    const only = parts.filter((x) => (typeof x === 'string' ? x.trim() !== '' : true));
    if (p && p.tagName === 'P' && only.length === 1 && typeof only[0] !== 'string' && only[0].classList.contains('math-display') && p.childNodes.length === 1) {
      p.replaceWith(only[0]);
    } else {
      node.replaceWith(...parts.filter((x) => x !== ''));
    }
  }
}

function copyButton(getText, onCopy) {
  const b = el('button', 'copy-btn', 'Copy');
  b.type = 'button';
  b.setAttribute('data-testid', 'copy-btn');
  b.addEventListener('click', async () => {
    const text = getText();
    try { await navigator.clipboard.writeText(text); b.textContent = 'Copied'; onCopy?.(text); } catch { b.textContent = 'Failed'; }
    setTimeout(() => { b.textContent = 'Copy'; }, 1400);
  });
  return b;
}

function wrapCode(pre, lang, onCopy) {
  const text = pre.textContent;
  const box = el('div', 'code-block');
  const head = el('div', 'code-head');
  head.append(el('span', 'code-lang', lang ?? 'text'), copyButton(() => text, onCopy));
  pre.replaceWith(box);
  box.append(head, pre);
  return box;
}

function highlightBlock(pre, code, onCopy) {
  const text = code.textContent;
  const r = highlight(text, fenceLang(code.className));
  if (r.html !== null) {
    code.innerHTML = DOMPurify.sanitize(r.html, { ALLOWED_TAGS: ['span'], ALLOWED_ATTR: ['class'] });
    code.classList.add('hljs');
  }
  return wrapCode(pre, r.lang, onCopy);
}

async function diagram(pre, code, id, onCopy) {
  const source = code.textContent;
  try {
    const svg = await renderDiagram(id, source);
    const box = el('figure', 'diagram');
    box.setAttribute('data-testid', 'mermaid-diagram');
    box.innerHTML = DOMPurify.sanitize(svg, SVG_PURIFY);
    if (!box.querySelector('svg')) throw new Error('empty diagram');
    pre.replaceWith(box);
  } catch (e) {
    const block = highlightBlock(pre, code, onCopy);
    const why = String(e?.message ?? 'error').split('\n')[0].slice(0, 120);
    block.after(el('p', 'render-note', `Diagram not shown (${why}). The source is above.`));
  }
}

function tidy(container) {
  for (const a of container.querySelectorAll('a')) {
    const href = a.getAttribute('href');
    if (href !== null && !SAFE_HREF.test(href.trim())) a.removeAttribute('href');
    if (a.hasAttribute('href') && !a.getAttribute('href').startsWith('#')) {
      a.setAttribute('target', '_blank');
      a.setAttribute('rel', 'noopener noreferrer');
    }
  }
  for (const t of container.querySelectorAll('table')) {
    const wrap = el('div', 'table-wrap');
    t.replaceWith(wrap);
    wrap.append(t);
  }
}

// renderAnswer(container, markdown, { onCopy }) -> Promise<void>; 
export async function renderAnswer(container, markdown, { onCopy } = {}) {
  const nonce = nonceOf();
  const { text, formulas } = extractLatex(markdown, nonce);
  const dirty = marked.parse(text, { async: false });
  inMarkdown = true;
  try { container.innerHTML = DOMPurify.sanitize(dirty, MARKDOWN_PURIFY); } finally { inMarkdown = false; }
  tidy(container);
  if (formulas.length) await fillFormulas(container, formulas, nonce);
  let k = 0;
  const jobs = [];
  for (const code of container.querySelectorAll('pre > code')) {
    const pre = code.parentElement;
    if (fenceLang(code.className) === 'mermaid') jobs.push(diagram(pre, code, `mmd-${nonce}-${k++}`, onCopy));
    else highlightBlock(pre, code, onCopy);
  }
  await Promise.all(jobs);
}
