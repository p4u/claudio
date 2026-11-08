import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { PROXY_TOKEN, PLUGIN_SECRET, PLUGIN } from '../fixtures/fake-proxy.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer({ proxy: true }); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('a plugin without choices opens setup before spawn and Skip spawns without it', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('n');
  await page.getByTestId('wizard-dir-input').fill(srv.dirs.a);
  await expect(page.getByTestId('wizard-proxy')).toHaveValue('default');
  await page.getByTestId('wizard-start').click();
  const setup = page.getByTestId('plugin-setup');
  await expect(setup).toBeVisible();
  await expect(setup).toContainText(PLUGIN);
  await page.getByTestId('plugin-skip').click();
  await expect(page.getByTestId('session-card')).toHaveCount(1);
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  const env = await srv.waitFor(() => srv.launchedEnv(srv.dirs.a), 'the launched env');
  expect(env).toContain(PROXY_TOKEN);
  expect(env).not.toContain(PLUGIN_SECRET);
  expect(env).not.toContain('VOCDONI_TEST_TOKEN');
});

test('proxy stats render the fixture window', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.b, { proxy: { kind: 'default' }, skip_plugins: [PLUGIN] });
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  await page.getByTestId('terminal').click();
  await page.keyboard.press('Control+Shift+Space');
  await page.keyboard.press('k');
  await page.getByTestId('palette-input').fill('stats');
  await page.getByTestId('palette-item').first().click();
  await page.getByTestId('stats-window-24h').click();
  const chart = page.getByTestId('stats-chart');
  await expect(chart).toBeVisible();
  await expect(chart.locator('title')).toContainText('claude-opus-5-5');
  await expect(page.getByTestId('stats')).toContainText('5.6k');
});
