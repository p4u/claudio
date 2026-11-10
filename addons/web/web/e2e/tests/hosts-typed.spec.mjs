import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

test('the wizard connects a host typed by hand and shows ssh\'s own error', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  await page.getByTestId('wizard-host').selectOption('other:');
  const input = page.getByTestId('wizard-host-input');
  const connect = page.getByTestId('wizard-host-connect');
  await expect(input).toBeVisible();
  await expect(connect).toBeDisabled();
  await input.fill('nonexistent.invalid');
  await expect(connect).toBeEnabled();
  await connect.click();
  const err = page.getByTestId('wizard-host-error');
  await expect(err).toContainText(/resolve|ssh failed/i, { timeout: 60_000 });
  expect((await err.textContent()).toLowerCase()).not.toContain('uname');
  await expect(input).toBeVisible();
  await expect(input).toBeEnabled();
  await expect(connect).toBeEnabled();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
