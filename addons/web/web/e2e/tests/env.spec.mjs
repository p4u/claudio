import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer({ envKeys: ['MY_GLOBAL', 'MY_SESSION', 'ANTHROPIC_DEFAULT_OPUS_MODEL'] }); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => {
  for (const name of ['MY_GLOBAL', 'ANTHROPIC_DEFAULT_OPUS_MODEL']) await srv.api('env.unset', { scope: 'global', name }).catch(() => {});
  await srv.reset();
});

const csp = (page) => page.evaluate(() => window.__claudioCsp ?? []);

test('the dialog adds a global and a session var, and a restart records them', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  const first = await srv.waitFor(() => srv.launchedEnv(srv.dirs.a), 'the first launch');
  expect(first).not.toContain('MY_GLOBAL');

  await page.keyboard.press('Alt+Comma');
  const dialog = page.getByTestId('env');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('Applies on next start or restart.');
  await expect(page.getByTestId('env-tab-global')).toHaveAttribute('aria-selected', 'true');
  await page.getByTestId('env-add-name').fill('MY_GLOBAL');
  await page.getByTestId('env-add-value').fill('one\ntwo');
  await page.getByTestId('env-add').click();
  const row = page.locator('[data-testid="env-row"][data-name="MY_GLOBAL"]');
  await expect(row.getByTestId('env-value')).toHaveValue('one\ntwo');
  await expect(page.getByTestId('env-add-name')).toHaveValue('');

  await page.getByTestId('env-tab-session').click();
  await expect(page.getByTestId('env-tab-session')).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('env-row')).toHaveCount(0);
  await page.getByTestId('env-add-name').fill('MY_SESSION');
  await page.getByTestId('env-add-value').fill('mine');
  await page.getByTestId('env-add').click();
  await expect(page.locator('[data-testid="env-row"][data-name="MY_SESSION"]')).toHaveCount(1);
  // A refused name says why and writes nothing.
  await page.getByTestId('env-add-name').fill('PATH');
  await page.getByTestId('env-add-value').fill('/x');
  await page.getByTestId('env-add').click();
  await expect(page.getByTestId('env-error')).toContainText('PATH: refused');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  await page.getByTestId('terminal').click();
  const before = srv.launchedEnv(srv.dirs.a);
  await page.keyboard.press('Alt+e');
  await page.getByTestId('confirm-yes').click();
  await srv.waitFor(() => { const e = srv.launchedEnv(srv.dirs.a); return e && e !== before && e; }, 'the restart');
  const env = srv.launchedEnv(srv.dirs.a);
  expect(env).toContain('MY_GLOBAL=one\ntwo');
  expect(env).toContain('MY_SESSION=mine');
  expect(await csp(page)).toEqual([]);
});

test('the catalogue datalist offers documented names with their description', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+Comma');
  await expect(page.getByTestId('env')).toBeVisible();
  await expect(page.getByTestId('env-tab-session')).toHaveCount(0);
  const option = page.locator('#env-names option[value="ANTHROPIC_DEFAULT_OPUS_MODEL"]');
  await expect(option).toHaveCount(1);
  await expect(option).toContainText('Model ID that the opus alias resolves to');
  await page.getByTestId('env-add-name').fill('ANTHROPIC_DEFAULT_OPUS_MODEL');
  await expect(page.getByTestId('env-add-desc')).toContainText('Model ID that the opus alias resolves to');
  await page.getByTestId('env-add-value').fill('claude-opus-5-5');
  await page.getByTestId('env-add').click();
  const row = page.locator('[data-testid="env-row"][data-name="ANTHROPIC_DEFAULT_OPUS_MODEL"]');
  await row.getByTestId('env-value').fill('claude-opus-5-6');
  await row.getByTestId('env-value').blur();
  await expect.poll(async () => (await srv.api('env.list', { scope: 'global' })).vars).toEqual([{ name: 'ANTHROPIC_DEFAULT_OPUS_MODEL', value: 'claude-opus-5-6' }]);
  // Escape in a value saves it before the dialog closes.
  await row.getByTestId('env-value').fill('claude-opus-5-7');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('env')).toHaveCount(0);
  await page.keyboard.press('Alt+Comma');
  await expect(page.locator('[data-testid="env-row"][data-name="ANTHROPIC_DEFAULT_OPUS_MODEL"]').getByTestId('env-value')).toHaveValue('claude-opus-5-7');
  await row.getByTestId('env-delete').click();
  await expect(page.getByTestId('env-row')).toHaveCount(0);
  expect(await csp(page)).toEqual([]);
});
