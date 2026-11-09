// Subsequence fuzzy matching for the command palette. Pure.
export function score(query, text) {
  const q = String(query).toLowerCase();
  const t = String(text).toLowerCase();
  if (!q) return 0;
  let ti = 0;
  let total = 0;
  let run = 0;
  for (let qi = 0; qi < q.length; qi++) {
    const at = t.indexOf(q[qi], ti);
    if (at < 0) return null;
    const contiguous = qi > 0 && at === ti;
    run = contiguous ? run + 1 : 0;
    total += 1 + run * 4;
    if (at === 0 || /[\s\-_/.:]/.test(t[at - 1])) total += 3;
    total -= Math.min(at - ti, 8) * 0.1;
    ti = at + 1;
  }
  return total - t.length * 0.01;
}

export function filter(items, query, key = (x) => x) {
  const scored = [];
  items.forEach((it, i) => {
    const s = score(query, key(it));
    if (s !== null) scored.push({ it, s, i });
  });
  scored.sort((a, b) => (b.s - a.s) || (a.i - b.i));
  return scored.map((x) => x.it);
}
