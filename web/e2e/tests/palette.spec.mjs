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

test('theme switch applies plain', async ({ page }) => {
  await page.goto(srv.url);
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByTestId('settings')).toBeVisible();
  await page.getByTestId('theme-select').selectOption('plain-dark');
  await expect(page.locator('html[data-theme^="plain"]')).toHaveCount(1);
});
