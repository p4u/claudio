// Matching the <latex> tags on a terminal screen to the session's formulas. Pure: imports
// nothing. A port of the TUI's src/tui/latex.rs (md_unescape, normalize, normalize_loose,
// without_html, Formula::new keys, matching, Row::tags, overlay_row, overlay_span), on JS strings.

// The most rows one tag may span after its first: claude wraps long lines itself, indenting
// the rows that continue one.
export const MAX_SPAN_ROWS = 8;

const ASCII_PUNCT = /^[!-/:-@[-`{-~]$/;
const BLANK = /\p{White_Space}/u;

// `s` without the backslash of each CommonMark escape (\{, \\, \,), as claude's markdown shows them.
export function mdUnescape(s) {
  const cs = Array.from(s);
  let out = '';
  for (let i = 0; i < cs.length; i++) {
    const c = cs[i];
    if (c !== '\\') { out += c; continue; }
    const next = cs[i + 1];
    if (next === undefined) { out += c; continue; }
    i++;
    out += ASCII_PUNCT.test(next) ? next : c + next;
  }
  return out;
}

// `s` without whitespace.
export function normalize(s) {
  return Array.from(s).filter((c) => !BLANK.test(c)).join('');
}

// normalize, also without the characters markdown may take as emphasis or code.
export function normalizeLoose(s) {
  return Array.from(s).filter((c) => !BLANK.test(c) && !'*_~`'.includes(c)).join('');
}

// `s` without the runs markdown may take for an HTML tag: `<` (or `</`), a letter, then anything
// but `<` up to the next `>`.
export function withoutHtml(s) {
  let out = '';
  let rest = s;
  for (;;) {
    const open = rest.indexOf('<');
    if (open < 0) break;
    out += rest.slice(0, open);
    const after = rest.slice(open + 1);
    const name = after.startsWith('/') ? after.slice(1) : after;
    let end = -1;
    if (/^[A-Za-z]/.test(name)) {
      const m = after.search(/[<>]/);
      if (m >= 0 && after[m] === '>') end = m;
    }
    if (end >= 0) rest = after.slice(end + 1);
    else { out += '<'; rest = after; }
  }
  return out + rest;
}

// -> {strict, loose, html}, as Formula::new builds them.
export function formulaKeys(src) {
  const u = mdUnescape(src);
  return { strict: normalize(u), loose: normalizeLoose(u), html: normalizeLoose(withoutHtml(u)) };
}

// The source of the newest formula (last in `formulas`) that a tag's on-screen `text` matches:
// strictly, then without emphasis characters, then also without HTML-looking runs. Or null.
// `keys` (optional) is a parallel array of formulaKeys, to spare recomputing them.
export function matchFormula(formulas, text, keys = null) {
  const k = keys ?? formulas.map(formulaKeys);
  const strict = normalize(text);
  const loose = normalizeLoose(text);
  const newest = (pick, want) => {
    for (let i = formulas.length - 1; i >= 0; i--) if (pick(k[i]) === want) return formulas[i];
    return null;
  };
  return newest((x) => x.strict, strict) ?? newest((x) => x.loose, loose) ?? newest((x) => x.html, loose);
}

// ---- screen rows: {glyphs: [{col, ch}], width} ----

const colOf = (row, i) => row.glyphs[i]?.col ?? row.width;

// The glyph index of the next `tag` (ASCII) at or after glyph `from`.
function find(row, tag, from) {
  const n = tag.length;
  for (let i = from; i + n <= row.glyphs.length; i++) {
    let ok = true;
    for (let j = 0; j < n; j++) if (row.glyphs[i + j].ch !== tag[j]) { ok = false; break; }
    if (ok) return i;
  }
  return -1;
}

// The text of glyphs from..to, trailing blanks dropped.
function textOf(row, from, to) {
  let s = '';
  for (let i = from; i < to; i++) s += row.glyphs[i].ch;
  return s.replace(/\p{White_Space}+$/u, '');
}

// The index of the first non-blank glyph.
function indent(row) {
  const i = row.glyphs.findIndex((g) => !BLANK.test(g.ch));
  return i < 0 ? row.glyphs.length : i;
}

// Every opening tag that starts on the row, in order: {open, close} glyph indexes (close -1 when
// the tag is not closed on the row; only the last can be).
function rowTags(row, open, close) {
  const found = [];
  let at = 0;
  for (;;) {
    const o = find(row, open, at);
    if (o < 0) break;
    const c = find(row, close, o + open.length);
    found.push({ open: o, close: c });
    if (c < 0) break;
    at = c + close.length;
  }
  return found;
}

// -> [{row, col, endRow, endCol, text, cont}]: the complete <tag>...</tag> of the screen.
// `col` is the column of the opening `<`, `endCol` the column past the closing `>`. `cont` lists
// the continuation rows of a multi-row span as {row, col, endCol}: the tag's text there, from the
// row's indentation (endCol is the row's width but on the last row, the span's endCol).
export function findSpans(rows, tag = 'latex') {
  const open = `<${tag}>`;
  const close = `</${tag}>`;
  const out = [];
  for (let r = 0; r < rows.length; r++) {
    const row = rows[r];
    const found = rowTags(row, open, close);
    if (!found.length) continue;
    if (found.length === 1 && found[0].close < 0) {
      const span = spanFrom(rows, r, found[0].open, open, close);
      if (span) { out.push(span); r = span.endRow; }
      continue;
    }
    for (const t of found) {
      if (t.close < 0) continue;
      out.push({
        row: r, col: colOf(row, t.open), endRow: r, endCol: colOf(row, t.close + close.length),
        text: textOf(row, t.open + open.length, t.close), cont: [],
      });
    }
  }
  return out;
}

// A tag opened at glyph `o` of row `r` (its only one), closing on one of the next rows.
function spanFrom(rows, r, o, open, close) {
  const first = rows[r];
  let text = textOf(first, o + open.length, first.glyphs.length);
  const cont = [];
  const stop = Math.min(rows.length, r + 1 + MAX_SPAN_ROWS);
  for (let last = r + 1; last < stop; last++) {
    const line = rows[last];
    if (find(line, open, 0) >= 0) return null; // another tag in the way
    const ind = indent(line);
    const c = find(line, close, ind);
    text += ' ' + textOf(line, ind, c < 0 ? line.glyphs.length : c);
    if (c >= 0) {
      const endCol = colOf(line, c + close.length);
      cont.push({ row: last, col: colOf(line, ind), endCol });
      return { row: r, col: colOf(first, o), endRow: last, endCol, text, cont };
    }
    cont.push({ row: last, col: colOf(line, ind), endCol: line.width });
  }
  return null;
}
