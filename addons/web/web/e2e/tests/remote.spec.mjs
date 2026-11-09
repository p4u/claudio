import { test, expect } from '@playwright/test';
import { startServer, TEST_PASSWORD } from '../fixtures/server.mjs';

// claudio.test resolves to 127.0.0.1 in this browser only: a non-local name.
test.use({ ignoreHTTPSErrors: true, launchOptions: { args: ['--host-resolver-rules=MAP claudio.test 127.0.0.1'] } });

let srv;
test.beforeAll(async () => { srv = await startServer({ remote: true, allowHosts: ['claudio.test'] }); });
test.afterAll(async () => { await srv?.stop(); });

test('over --remote the page asks for the password and the session works', async ({ page }) => {
  expect(srv.url).toBe(`https://127.0.0.1:${srv.port}/`);
  await page.goto(srv.url);
  await expect(page.getByTestId('login-form')).toBeVisible();
  await page.getByTestId('login-password').fill('not-the-password');
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('login-error')).toHaveText('Wrong password.');
  await expect(page.getByTestId('login-password')).toHaveValue('');
  await page.getByTestId('login-password').fill(TEST_PASSWORD);
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('rail')).toBeVisible();
  const sid = await srv.spawn(srv.dirs.a);
  await expect(page.locator(`[data-testid="session-card"][data-sid="${sid}"]`)).toBeVisible();
  expect(await page.evaluate(() => window.isSecureContext)).toBe(true);
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
  const cookie = (await page.context().cookies()).find((c) => c.name === 'claudio_web');
  expect(cookie?.secure).toBe(true);
  expect(cookie?.httpOnly).toBe(true);
  const stored = await page.evaluate(() => JSON.stringify(localStorage) + JSON.stringify(sessionStorage));
  expect(stored).not.toContain(TEST_PASSWORD);
  await srv.reset();
});

test('a token link on another name falls back to the password', async ({ page }) => {
  await page.goto(`https://claudio.test:${srv.port}/#t=${srv.token}`);
  await expect(page.getByTestId('login-form')).toBeVisible();
  await expect(page.getByTestId('login-error')).toContainText('only on the machine');
  expect(await page.evaluate(() => location.hash)).toBe('');
  await page.getByTestId('login-password').fill(TEST_PASSWORD);
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('rail')).toBeVisible();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
