// Loads a classic script or a stylesheet once (KaTeX, highlight.js, mermaid, ...).
const loaded = new Map();

function once(key, make) {
  if (!loaded.has(key)) {
    loaded.set(key, make().catch((e) => { loaded.delete(key); throw e; }));
  }
  return loaded.get(key);
}

export function loadScript(src) {
  return once(`s:${src}`, () => new Promise((resolve, reject) => {
    const el = document.createElement('script');
    el.src = src;
    el.async = true;
    el.onload = () => resolve();
    el.onerror = () => reject(new Error(`cannot load ${src}`));
    document.head.append(el);
  }));
}

export function loadStyle(href) {
  return once(`c:${href}`, () => new Promise((resolve, reject) => {
    const el = document.createElement('link');
    el.rel = 'stylesheet';
    el.href = href;
    el.onload = () => resolve();
    el.onerror = () => reject(new Error(`cannot load ${href}`));
    document.head.append(el);
  }));
}
