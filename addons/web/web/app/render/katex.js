// KaTeX, loaded on first use from /vendor (a classic script plus its stylesheet).
import { loadScript, loadStyle } from '../lazy.js';

export async function getKatex() {
  await Promise.all([loadStyle('/vendor/katex/katex.min.css'), loadScript('/vendor/katex/katex.min.js')]);
  if (!globalThis.katex) throw new Error('katex did not load');
  return globalThis.katex;
}
