// highlight.js (common languages). The token colours are in components.css (--hl-* in tokens.css).
import hljs from 'highlight.js';

const AUTO_MAX = 20000; // auto-detection tries every language: keep it for short snippets

// -> { html, lang }: markup made only of <span class="hljs-…">, to be purified by the caller.
export function highlight(text, lang) {
  try {
    if (lang && hljs.getLanguage(lang)) return { html: hljs.highlight(text, { language: lang, ignoreIllegals: true }).value, lang };
    if (!lang && text.length <= AUTO_MAX) {
      const r = hljs.highlightAuto(text);
      return { html: r.value, lang: r.language ?? null };
    }
  } catch { /* fall through to plain text */ }
  return { html: null, lang };
}
