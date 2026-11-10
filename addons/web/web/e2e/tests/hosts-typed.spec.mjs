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

test('Enter in the typed host connects it and starts no session on the host shown before', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  // A folder makes the wizard ready: Enter in any of its other text fields starts the session.
  await page.getByTestId('wizard-dir-input').fill(srv.dirs.a);
  const start = page.getByTestId('wizard-start');
  await expect(start).toBeEnabled();
  await page.getByTestId('wizard-host').selectOption('other:');
  // The form no longer shows a host: nothing starts until one is connected or picked.
  await expect(start).toBeDisabled();
  // Nor does Enter in another text field, the wizard's keyboard Start.
  await page.getByTestId('wizard-name').press('Enter');
  const input = page.getByTestId('wizard-host-input');
  await input.fill('nonexistent.invalid');
  await input.press('Enter');
  await expect(page.getByTestId('wizard-host-error')).toContainText(/resolve|ssh failed/i, { timeout: 60_000 });
  await expect(page.getByTestId('wizard')).toBeVisible();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
  await expect(start).toBeDisabled();
  await page.getByTestId('wizard-host').selectOption('local');
  await expect(input).toHaveCount(0);
  await expect(start).toBeEnabled();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
