// mermaid, loaded on first use. securityLevel strict and SVG labels (MERMAID_CONFIG); the theme
// variables come from the palette tokens (tokens.css): Carbon nodes, Slate lines, Silver text and a
// Phosphor accent, on the 'base' theme.
import { MERMAID_CONFIG } from './markdown.js';

let lib = null;
let themeKey = null;
let chain = Promise.resolve(); // mermaid renders one diagram at a time

// Plain palette tokens only: mermaid parses the colours itself.
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

function themeVariables() {
  const carbon = css('--carbon');
  const graphite = css('--graphite');
  const void_ = css('--void');
  const line = css('--steel');
  const border = css('--fog');
  const text = css('--silver');
  const strong = css('--bone');
  const accent = css('--phosphor');
  return {
    darkMode: true,
    background: carbon,
    fontFamily: css('--font-sans') || 'sans-serif',
    fontSize: '15px',
    primaryColor: graphite,
    primaryTextColor: strong,
    primaryBorderColor: accent,
    secondaryColor: carbon,
    secondaryTextColor: text,
    secondaryBorderColor: border,
    tertiaryColor: void_,
    tertiaryTextColor: text,
    tertiaryBorderColor: border,
    lineColor: line,
    textColor: text,
    mainBkg: graphite,
    nodeBorder: accent,
    clusterBkg: carbon,
    clusterBorder: border,
    edgeLabelBackground: carbon,
    noteBkgColor: graphite,
    noteTextColor: text,
    noteBorderColor: border,
    actorBkg: graphite,
    actorBorder: accent,
    actorTextColor: strong,
    actorLineColor: line,
    signalColor: text,
    signalTextColor: text,
    labelBoxBkgColor: graphite,
    labelBoxBorderColor: border,
    labelTextColor: text,
    loopTextColor: text,
    titleColor: strong,
  };
}

async function ready() {
  if (!lib) lib = (await import('/vendor/mermaid/mermaid.esm.min.mjs')).default;
  const key = document.documentElement.dataset.theme;
  if (key !== themeKey) {
    lib.initialize({ ...MERMAID_CONFIG, theme: 'base', themeVariables: themeVariables() });
    themeKey = key;
  }
  return lib;
}

// -> the SVG text (still to be purified). Rejects when the diagram does not parse or its type is missing.
export function renderDiagram(id, source) {
  const run = async () => {
    const m = await ready();
    try {
      const { svg } = await m.render(id, source);
      return svg;
    } finally {
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
  };
  const p = chain.then(run, run);
  chain = p.catch(() => {});
  return p;
}
