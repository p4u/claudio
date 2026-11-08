// Pure part of the answer renderer: sanitizer configs and the <latex> preprocessing. Imports nothing.
// Every config here is the one docs/web-protocol.md §7 asks for; tests pin them.

export const MARKDOWN_PURIFY = {
  FORBID_TAGS: ['style', 'script', 'iframe', 'form', 'input', 'object', 'embed', 'link', 'meta', 'base'],
  FORBID_ATTR: ['style'],
  ALLOW_DATA_ATTR: false,
};
export const SVG_PURIFY = { USE_PROFILES: { svg: true, svgFilters: true } };
export const KATEX_OPTIONS = { trust: false, maxSize: 20, maxExpand: 1000, throwOnError: true, strict: 'ignore', output: 'htmlAndMathml' };
export const MERMAID_CONFIG = { startOnLoad: false, securityLevel: 'strict', htmlLabels: false, flowchart: { htmlLabels: false } };

// Placeholders are wrapped in private-use characters. The daemon's sanitize strips them from model
// text; this module drops any that remain, so the model cannot forge one for a nonce it never saw.
const OPEN = '\uE000';
const CLOSE = '\uE001';
const PRIVATE = /[\uE000-\uF8FF]/g;

const esc = (s) => String(s).replace(/[^A-Za-z0-9_-]/g, '');

export function placeholderRe(nonce) {
  return new RegExp(`${OPEN}${esc(nonce)}:(\\d+)${CLOSE}`, 'g');
}

export function fenceLang(codeClassName) {
  const m = /(?:^|\s)language-([\w+#.-]+)/.exec(codeClassName ?? '');
  return m ? m[1].toLowerCase() : null;
}

// Ranges [start, end) of fenced code blocks and inline code spans.
function codeRanges(md) {
  const ranges = [];
  const lines = md.split('\n');
  let pos = 0;
  let open = null; // { start, ch, len }
  const fenced = [];
  for (const line of lines) {
    const end = pos + line.length + 1;
    if (open) {
      const m = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(line);
      if (m && m[1][0] === open.ch && m[1].length >= open.len) {
        fenced.push([open.start, Math.min(end, md.length)]);
        open = null;
      }
    } else {
      const m = /^ {0,3}(`{3,}|~{3,})/.exec(line);
      if (m && !(m[1][0] === '`' && line.slice(m[0].length).includes('`'))) open = { start: pos, ch: m[1][0], len: m[1].length };
    }
    pos = end;
  }
  if (open) fenced.push([open.start, md.length]);
  ranges.push(...fenced);
  // inline code, outside the fences
  const inFence = (i) => fenced.some(([a, b]) => i >= a && i < b);
  const re = /(?<!`)(`+)(?!`)[\s\S]*?[^`]\1(?!`)/g;
  for (const m of md.matchAll(re)) if (!inFence(m.index)) ranges.push([m.index, m.index + m[0].length]);
  return ranges;
}

/// `<latex>…</latex>` outside code → placeholders; a tag alone on its line is display math.
export function extractLatex(markdown, nonce) {
  const md = String(markdown).replace(PRIVATE, '');
  const ranges = codeRanges(md);
  const inCode = (i) => ranges.some(([a, b]) => i >= a && i < b);
  const formulas = [];
  let out = '';
  let at = 0;
  const re = /<latex>([\s\S]*?)<\/latex>/g;
  for (const m of md.matchAll(re)) {
    if (inCode(m.index)) continue;
    const lineStart = md.lastIndexOf('\n', m.index - 1) + 1;
    let lineEnd = md.indexOf('\n', m.index + m[0].length);
    if (lineEnd < 0) lineEnd = md.length;
    const alone = /^[ \t]*$/.test(md.slice(lineStart, m.index)) && /^[ \t]*$/.test(md.slice(m.index + m[0].length, lineEnd));
    const tex = m[1].trim();
    const ph = `${OPEN}${esc(nonce)}:${formulas.length}${CLOSE}`;
    formulas.push({ tex, display: alone });
    if (alone) { // the whole line, so an indented tag is not read as an indented code block
      out += md.slice(at, lineStart) + ph;
      at = lineEnd;
    } else {
      out += md.slice(at, m.index) + ph;
      at = m.index + m[0].length;
    }
  }
  return { text: out + md.slice(at), formulas };
}

// Raw HTML from the model, before DOMPurify: drops on*= attributes. DOMPurify would remove them
// anyway; this keeps the browser from reporting a CSP violation while it parses them (the e2e
// asserts there are none), and is only a first pass. DOMPurify stays the guarantee.
export function stripEventHandlers(raw) {
  const re = /(<[a-zA-Z][^<>]*?)[\s/]+on[a-zA-Z]+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'<>]+)/g;
  let cur = String(raw);
  for (let i = 0; i < 20; i++) {
    const next = cur.replace(re, '$1');
    if (next === cur) break;
    cur = next;
  }
  return cur;
}
