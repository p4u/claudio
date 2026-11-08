// Context gauge helpers. Imports nothing.
export function segments(fraction, n = 8) {
  const f = Number.isFinite(fraction) ? Math.min(1, Math.max(0, fraction)) : 0;
  const filled = Math.round(f * n);
  return '█'.repeat(filled) + '░'.repeat(n - filled);
}

export function level(fraction) {
  return fraction >= 0.85 ? 'hot' : fraction >= 0.6 ? 'warn' : 'ok';
}

// context_tokens over the model's window: 1M when the name says [1m], else 200k.
export function ctxFraction(session, model) {
  const tokens = session?.context_tokens;
  if (!Number.isFinite(tokens)) return null;
  const name = String(model ?? session.model ?? '');
  const window = /\[1m\]/i.test(name) ? 1_000_000 : 200_000;
  return Math.min(1, tokens / window);
}
