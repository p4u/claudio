// Context gauge helpers. Imports nothing.
export function segments(fraction, n = 8) {
  const f = Number.isFinite(fraction) ? Math.min(1, Math.max(0, fraction)) : 0;
  const filled = Math.round(f * n);
  return '█'.repeat(filled) + '░'.repeat(n - filled);
}

export function level(fraction) {
  return fraction >= 0.85 ? 'hot' : fraction >= 0.6 ? 'warn' : 'ok';
}

// The window claude compacts against (the same rule as src/tui/ui/status.rs context_window):
// claude has two windows, 200k and 1M, and the transcript never says which one a session has.
// 1M for a model id with a `1m` token ([1m], -1m), for Opus 5.x (Claude Code runs it at 1M by
// default), and whenever the context used is already past 200k (so the gauge never reads over
// 100 %); else 200k.
export const SMALL_WINDOW = 200_000;
export const LARGE_WINDOW = 1_000_000;
export function contextWindow(model, used = 0) {
  const name = String(model ?? '');
  const oneM = name.split(/[^A-Za-z0-9]/).some((p) => p.toLowerCase() === '1m');
  const opus5 = name.toLowerCase().startsWith('claude-opus-5');
  return oneM || opus5 || (Number.isFinite(used) && used > SMALL_WINDOW) ? LARGE_WINDOW : SMALL_WINDOW;
}

// context_tokens over the session's window.
export function ctxFraction(session, model) {
  const tokens = session?.context_tokens;
  if (!Number.isFinite(tokens)) return null;
  return Math.min(1, tokens / contextWindow(model ?? session.model, tokens));
}
