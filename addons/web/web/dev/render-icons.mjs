// Renders the app's PNG icons from their SVG sources with Playwright's Chromium (headless, a
// throwaway profile), once; the PNGs are committed. From the repository root:
//   (cd addons/web/web/e2e && npm ci) && node addons/web/web/dev/render-icons.mjs
//   icons/claudio.svg            -> icons/claudio-192.png, icons/claudio-512.png (purpose "any")
//   dev/icons/claudio-maskable.svg -> icons/claudio-maskable-512.png (purpose "maskable": a
//   full-bleed square, the glyph scaled 0.8 about the centre, inside the 40 % safe circle)
import fs from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../e2e/package.json', import.meta.url));
const { chromium } = require('@playwright/test');
const web = (rel) => fileURLToPath(new URL(`../${rel}`, import.meta.url));

const jobs = [
  ['icons/claudio.svg', 'icons/claudio-192.png', 192],
  ['icons/claudio.svg', 'icons/claudio-512.png', 512],
  ['dev/icons/claudio-maskable.svg', 'icons/claudio-maskable-512.png', 512],
];

const browser = await chromium.launch();
try {
  for (const [src, out, size] of jobs) {
    const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
    const svg = fs.readFileSync(web(src)).toString('base64');
    await page.setContent(`<html><body style="margin:0;background:transparent"><img style="display:block" width="${size}" height="${size}" src="data:image/svg+xml;base64,${svg}"></body></html>`);
    await page.locator('img').evaluate((img) => img.decode());
    await page.screenshot({ path: web(out), omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
    await page.close();
    console.log(`${out}: ${size}x${size}, ${fs.statSync(web(out)).size} bytes`);
  }
} finally {
  await browser.close();
}
