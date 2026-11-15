// The custom env dialog's pure part (ui/env.js draws it). Imports nothing.

const NAME = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

/** Whether `name` is a variable name the server takes (before the refused set). */
export function nameOk(name) {
  return NAME.test(name);
}

/** A value as the server gets it: `\r\n` (and a lone `\r`) as `\n`. */
export function normalizeValue(text) {
  return String(text).replace(/\r\n?/g, '\n');
}

/** The rows a value's textarea shows: one per line, 1 to 8. */
export function valueRows(text) {
  return Math.min(8, Math.max(1, String(text).split('\n').length));
}

/**
 * What the add row says about `name` (trimmed): `{refused}` with the reason, `{bad}` for a
 * malformed one, `{known}` with the catalogue entry, or `{}` (any other name is fine).
 */
export function nameNote(catalogue, name) {
  const n = String(name).trim();
  if (!n) return {};
  if (!nameOk(n)) return { bad: true };
  const refused = (catalogue.refused ?? []).find((r) => r.name === n);
  if (refused) return { refused: refused.reason };
  const known = (catalogue.vars ?? []).find((v) => v.name === n);
  return known ? { known } : {};
}

/** The `env.*` scope args of a tab: `global`, or `session` with the focused claude session. */
export function scopeArgs(tab, sid) {
  return tab === 'session' && sid ? { scope: 'session', sid } : { scope: 'global' };
}
