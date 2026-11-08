import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

test('the Hosts button opens a menu of hosts, keyboard and outside click close it', async ({ page }) => {
  await page.goto(srv.url);
  const btn = page.getByTestId('hosts-btn');
  await expect(btn).toContainText('Hosts');
  await expect(page.getByTestId('hosts-summary')).toHaveText('1/1');
  await expect(page.getByTestId('hosts-menu')).toHaveCount(0);
  const before = await page.locator('.topbar').boundingBox();
  await btn.focus();
  await page.keyboard.press('Enter');
  const menu = page.getByTestId('hosts-menu');
  await expect(menu).toBeVisible();
  expect((await page.locator('.topbar').boundingBox()).height).toBe(before.height);
  await expect(menu.locator('[data-testid="host-chip"][data-host="local"]')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByTestId('host-add-chip')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(btn).toBeFocused();
  await btn.click();
  await expect(menu).toBeVisible();
  await page.mouse.click(700, 500);
  await expect(menu).toHaveCount(0);
  await btn.click();
  await menu.getByTestId('host-add-chip').click();
  await expect(menu).toHaveCount(0);
  await expect(page.getByTestId('hosts-panel').or(page.getByRole('dialog')).first()).toBeVisible();
});
