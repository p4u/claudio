import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('Changes lists a modified file and shows its diff', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.repo);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('pane')).toHaveAttribute('data-state', /./);
  await page.keyboard.press('g');
  await expect(page.getByTestId('drawer')).toBeVisible();
  const file = page.locator('[data-testid="changes-file"][data-path="a.txt"]');
  await expect(file).toBeVisible();
  await file.click();
  await expect(page.getByTestId('diff')).toContainText('+changed');
});
