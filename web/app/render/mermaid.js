// mermaid, loaded on first use. securityLevel strict and SVG labels (MERMAID_CONFIG); the theme
// variables come from the app's CSS tokens so diagrams match arcade-dark/light and the plain themes.
import { MERMAID_CONFIG } from './markdown.js';

let lib = null;
let themeKey = null;
let chain = Promise.resolve(); // mermaid renders one diagram at a time

const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

function themeVariables() {
  const dark = document.documentElement.dataset.theme?.endsWith('dark') !== false;
  return {
    darkMode: dark,
    background: css('--panel'),
    fontFamily: css('--font-prose') || 'sans-serif',
    fontSize: '15px',
    primaryColor: css('--panel-2'),
    primaryTextColor: css('--text'),
    primaryBorderColor: css('--accent'),
    secondaryColor: css('--panel'),
    secondaryTextColor: css('--text'),
    secondaryBorderColor: css('--border-strong'),
    tertiaryColor: css('--bg'),
    tertiaryTextColor: css('--text'),
    tertiaryBorderColor: css('--border-strong'),
    lineColor: css('--muted'),
    textColor: css('--text'),
    mainBkg: css('--panel-2'),
    nodeBorder: css('--accent'),
    clusterBkg: css('--panel'),
    clusterBorder: css('--border-strong'),
    edgeLabelBackground: css('--panel'),
    noteBkgColor: css('--panel-2'),
    noteTextColor: css('--text'),
    noteBorderColor: css('--border-strong'),
    actorBkg: css('--panel-2'),
    actorBorder: css('--accent'),
    actorTextColor: css('--text'),
    actorLineColor: css('--muted'),
    signalColor: css('--text'),
    signalTextColor: css('--text'),
    labelBoxBkgColor: css('--panel-2'),
    labelBoxBorderColor: css('--border-strong'),
    labelTextColor: css('--text'),
    loopTextColor: css('--text'),
    titleColor: css('--text'),
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
