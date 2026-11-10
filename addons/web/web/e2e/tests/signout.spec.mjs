import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

test('the palette signs this browser out, and its cookie stays refused', async ({ page, request }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  const cookie = (await page.context().cookies()).find((c) => c.name === 'claudio_web');
  expect(cookie?.value).toMatch(/^[0-9a-f]{64}$/);
  // Max-Age=34560000: it outlives the browser (400 days, not a session cookie).
  expect(cookie.expires).toBeGreaterThan(Date.now() / 1000 + 399 * 24 * 3600);
  const me = () => request.get(`http://127.0.0.1:${srv.port}/api/me`, { headers: { Cookie: `claudio_web=${cookie.value}` } });
  expect((await me()).status()).toBe(204);
  await page.keyboard.press('Control+k');
  await page.getByTestId('palette-input').fill('Sign out');
  await page.getByTestId('palette-item').first().click();
  await expect(page.getByText('Open the URL')).toBeVisible();
  expect((await page.context().cookies()).find((c) => c.name === 'claudio_web')).toBeUndefined();
  expect((await me()).status()).toBe(401);
  await page.reload();
  await expect(page.getByText('Open the URL')).toBeVisible();
  expect((await me()).status()).toBe(401);
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
