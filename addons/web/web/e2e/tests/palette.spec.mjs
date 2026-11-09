import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('Ctrl+Shift+Space then k opens the palette', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const term = page.getByTestId('terminal');
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  await term.click();
  await expect(page.getByTestId('mode-badge')).toHaveText('TERM');
  await page.keyboard.press('Control+Shift+Space');
  await expect(page.getByTestId('mode-badge')).toHaveText('APP');
  await page.keyboard.press('k');
  await expect(page.getByTestId('palette')).toBeVisible();
  await expect(page.getByTestId('palette-item').first()).toBeVisible();
});

// Was "theme switch applies plain": the theme setting is gone (the page is dark only and follows
// DESIGN.md), so this checks the design's invariants instead: the Void canvas, a ghost button with a
// 1px radius and no shadow, the logo's Phosphor accent, and weight 400 across the app chrome.
test('the page follows DESIGN.md', async ({ page }) => {
  await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await expect(page.getByTestId('session-card').first()).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(11, 14, 18)');
  const ghost = page.getByTestId('new-session-btn');
  await expect(ghost).toHaveCSS('border-radius', '1px');
  await expect(ghost).toHaveCSS('box-shadow', 'none');
  await expect(page.locator('.logo-accent')).toHaveCSS('color', 'rgb(0, 216, 146)');
  await page.getByTestId('session-card').first().click();
  await page.keyboard.press('r');
  await expect(page.getByTestId('drawer')).toBeVisible();
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByTestId('settings')).toBeVisible();
  await expect(page.getByTestId('theme-select')).toHaveCount(0);
  // every element of the chrome (top bar, rail, stage bar, pane headers, drawer, footer, an open
  // dialog) is weight 400; only the terminal's bold cells (and KaTeX's own \\mathbf) may be bold
  const heavy = await page.evaluate(() => [...document.querySelectorAll('.topbar *, .rail *, .stage-bar *, .pane-head *, .drawer *, .footer *, [role="dialog"] *')]
    .filter((el) => !el.closest('.katex, .xterm') && Number(getComputedStyle(el).fontWeight) > 400)
    .map((el) => `${el.tagName.toLowerCase()}.${el.className}`));
  expect(heavy).toEqual([]);
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
