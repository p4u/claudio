// Chart arithmetic for the stats drawer. Imports nothing.

// linearScale([d0, d1], [r0, r1]) -> (v) => r. A flat domain maps everything to r0.
export function linearScale([d0, d1], [r0, r1]) {
  const span = d1 - d0;
  return (v) => (span === 0 ? r0 : r0 + ((v - d0) / span) * (r1 - r0));
}

// Round tick values from 0 up to (at least) max, about `count` intervals.
export function niceTicks(max, count = 4) {
  const top = Number.isFinite(max) && max > 0 ? max : 0;
  if (top === 0) return [0, 1];
  const raw = top / Math.max(1, count);
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const steps = mag >= 10 ? [1, 2, 2.5, 5, 10] : [1, 2, 5, 10];
  const step = (steps.find((s) => norm <= s) ?? 10) * mag;
  const out = [];
  for (let v = 0; v < top + step * 0.999; v += step) {
    out.push(Math.round(v * 1e6) / 1e6);
    if (v >= top) break;
  }
  return out;
}

// One bar per value, left to right across `width`, tallest = `height`. Zero is a flat bar.
export function barLayout(values, width, height, gap = 2) {
  const n = values.length;
  if (n === 0) return [];
  const w = Math.max(0, (width - gap * (n - 1)) / n);
  const max = Math.max(...values, 0);
  return values.map((v, i) => {
    const h = max > 0 ? (Math.max(0, v) / max) * height : 0;
    return { x: i * (w + gap), y: height - h, w, h };
  });
}

const trim = (s) => s.replace(/\.0$/, '');

// 950 -> '950', 5600 -> '5.6k', 2_300_000 -> '2.3M'
export function formatTokens(n) {
  if (!Number.isFinite(n)) return '–';
  const a = Math.abs(n);
  if (a >= 1e6) return `${trim((n / 1e6).toFixed(1))}M`;
  if (a >= 1e5) return `${Math.round(n / 1e3)}k`;
  if (a >= 1e3) return `${trim((n / 1e3).toFixed(1))}k`;
  return String(Math.round(n));
}
