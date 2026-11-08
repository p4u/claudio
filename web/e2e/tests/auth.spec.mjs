import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

test('logs in from the token URL and clears the fragment', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await expect(page.getByTestId('board')).toBeVisible();
  expect(await page.evaluate(() => location.hash)).toBe('');
  await page.goto(`http://127.0.0.1:${srv.port}/`);
  await expect(page.getByTestId('rail')).toBeVisible();
  await expect(page.getByTestId('board')).toBeVisible();
  expect(await page.evaluate(() => location.hash)).toBe('');
});
