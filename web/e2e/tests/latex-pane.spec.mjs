import os from 'node:os';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { writeTranscript } from '../fixtures/transcript.mjs';

const SHOTS = path.join(os.homedir(), '.cache/claudio/web-ui/shots/latex-overlay');
let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

async function openSession(page) {
  writeTranscript(srv.claudeConfig, srv.dirs.a, 'conv-latex');
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'SessionStart', { session_id: 'conv-latex', source: 'startup' });
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  await page.getByTestId('terminal').click();
  return sid;
}

test('a matched <latex> tag in a pane is typeset over its text, and the setting turns it off', async ({ page }) => {
  await openSession(page);
  await page.keyboard.type('<latex>E = mc^2</latex>');
  await page.keyboard.press('Enter');
  const layer = page.getByTestId('terminal').getByTestId('latex-overlay');
  await expect(layer.locator('.katex').first()).toBeVisible();
  // an unknown formula is left as typed
  await page.keyboard.type('<latex>q + z</latex>');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('terminal')).toContainText('line:<latex>q + z</latex>');
  await page.waitForTimeout(300);
  await expect(layer.locator('.latex-box')).not.toHaveCount(0);
  for (const [theme, name] of [['arcade-dark', 'dark'], ['arcade-light', 'light']]) {
    await page.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme);
    await page.waitForTimeout(400);
    await page.getByTestId('terminal').screenshot({ path: path.join(SHOTS, `pane-${name}.png`) });
  }
  await page.evaluate(() => document.documentElement.setAttribute('data-theme', 'arcade-dark'));

  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByTestId('render-formulas').uncheck();
  await expect(layer.locator('.latex-box')).toHaveCount(0);
  await expect(layer.locator('.katex')).toHaveCount(0);
  await page.getByTestId('render-formulas').check();
  await expect(layer.locator('.katex').first()).toBeVisible();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
