import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('the wizard starts a session in a chosen dir', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  await page.getByTestId('wizard-dir-input').fill(srv.dirs.b);
  await page.getByTestId('wizard-proxy').selectOption('direct');
  await page.getByTestId('wizard-start').click();
  const card = page.getByTestId('session-card');
  await expect(card).toHaveCount(1);
  // the folder is in the hover info (the card itself shows name, host and age only)
  await card.hover();
  await expect(page.getByTestId('session-info')).toContainText(srv.dirs.b);
  await expect(card).toContainText('b');
  await expect(page.locator('[data-testid="pane"][data-focus="true"]').getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
});
