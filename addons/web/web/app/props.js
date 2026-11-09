// Spreading two prop objects on one element lets the later `on*` replace the earlier one. This
// merges them instead: same-named handlers run in order; other keys take the last value.
export function mergeHandlers(...objs) {
  const out = {};
  for (const o of objs) {
    for (const [k, v] of Object.entries(o ?? {})) {
      const prev = out[k];
      if (/^on[A-Z]/.test(k) && typeof v === 'function' && typeof prev === 'function') {
        out[k] = function merged(...args) {
          prev.apply(this, args);
          return v.apply(this, args);
        };
      } else out[k] = v;
    }
  }
  return out;
}
