// What the page draws of addon data (docs/web-protocol.md §4 "Addons", §7), pure. Imports nothing.

export const TONES = ['ok', 'warn', 'hot', 'muted', 'accent'];
export const tone = (t) => (TONES.includes(t) ? t : 'muted');

// A session's segments by priority (lower first), ties in the order they came.
export function sortSegments(segments) {
  return (segments ?? []).map((s, i) => [s, i])
    .sort((a, b) => ((a[0].priority ?? 255) - (b[0].priority ?? 255)) || (a[1] - b[1]))
    .map(([s]) => s);
}

const num = (n) => (Number.isFinite(n) ? n : 0);

// A bars block's entries with their width: a percentage of the largest value (0 when all are 0).
export function barRows(series) {
  const rows = (series ?? []).map(([label, v]) => ({ label: String(label), value: num(v) }));
  const top = Math.max(0, ...rows.map((r) => r.value));
  return rows.map((r) => ({ ...r, pct: top > 0 ? Math.max(0, Math.min(100, (100 * r.value) / top)) : 0 }));
}

// A gauge block: used / limit as a percentage (0..100) and a level: ok, warn (>= 60), hot (>= 85).
export function gaugeOf(block) {
  const used = num(block.used);
  const limit = num(block.limit);
  const pct = limit > 0 ? Math.max(0, Math.min(100, (100 * used) / limit)) : 0;
  return { used, limit, pct, level: pct >= 85 ? 'hot' : pct >= 60 ? 'warn' : 'ok' };
}

// 1234 -> '1.2k', 5_600_000 -> '5.6M': a gauge's numbers.
export function short(n) {
  const v = num(n);
  const a = Math.abs(v);
  if (a >= 1e9) return `${(v / 1e9).toFixed(1)}G`;
  if (a >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
  if (a >= 1e3) return `${(v / 1e3).toFixed(1)}k`;
  return String(Math.round(v));
}

// Checklist items grouped where their `section` changes, in their order: [{section, items}].
export function sections(items) {
  const out = [];
  for (const it of items ?? []) {
    const sec = it.section ?? null;
    const last = out[out.length - 1];
    if (last && last.section === sec) last.items.push(it);
    else out.push({ section: sec, items: [it] });
  }
  return out;
}

// The ids a checklist starts with off.
export const initialOff = (items) => new Set((items ?? []).filter((i) => !i.on).map((i) => i.id));

// The panel decl a key spec opens ('alt+s'), else the first panel of any addon: {addon, panel}.
export function panelFor(decls, key = null) {
  let first = null;
  for (const d of decls ?? []) {
    for (const p of d.panels ?? []) {
      if (key && p.key === key) return { addon: d.name, panel: p };
      if (!first) first = { addon: d.name, panel: p };
    }
  }
  return first;
}
