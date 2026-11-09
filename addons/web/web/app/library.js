// The prompt library: claudio's snippets, named. Pure helpers; imports nothing.
const MAX_SLUG = 40;

// The name the daemon files a snippet under (src/tui/snippets.rs `slug`): the first words, ASCII
// lowercase letters and digits joined by `-`, at most 40 characters; `snippet` when none is left.
export function slug(text) {
  let out = '';
  for (const w of String(text).split(/[^A-Za-z0-9]+/).filter(Boolean)) {
    const word = w.toLowerCase();
    const sep = out ? 1 : 0;
    if (out.length + sep + word.length > MAX_SLUG) {
      if (!out) out = word.slice(0, MAX_SLUG);
      break;
    }
    out += (sep ? '-' : '') + word;
  }
  return out || 'snippet';
}

// The first non-blank line, trimmed and cut with an ellipsis at `max` characters.
export function firstLine(text, max = 80) {
  const line = String(text ?? '').split('\n').map((l) => l.trim()).find(Boolean) ?? '';
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

// A name to pre-fill "Save as…": the first line without its closing punctuation.
export function nameFromText(text) {
  const line = firstLine(text, 40).replace(/[.:;,!?…\s]+$/, '');
  return line;
}

export function filterItems(items, query) {
  const q = String(query ?? '').trim().toLowerCase();
  if (!items) return [];
  if (!q) return items;
  return items.filter((i) => i.name.toLowerCase().includes(q) || i.text.toLowerCase().includes(q));
}

// The saved prompt that saving under `name` would replace, or null.
export function hasName(items, name) {
  const s = slug(name);
  return (items ?? []).find((i) => i.name === s) ?? null;
}
