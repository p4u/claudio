import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { writeTranscript } from '../fixtures/transcript.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('the reader renders KaTeX and mermaid with no CSP violation', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.a, 'conv-reader');
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'SessionStart', { session_id: 'conv-reader', source: 'startup' });
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  await page.getByTestId('terminal').click();
  await page.keyboard.press('Control+Shift+Space');
  await page.keyboard.press('r');
  const answer = page.getByTestId('drawer').getByTestId('reader-answer').first();
  await expect(answer).toContainText('fixtureword');
  await expect(answer.locator('.katex-display')).toHaveCount(1);
  await expect(answer.locator('.katex:not(.katex-display .katex)')).toHaveCount(1);
  await expect(answer.getByTestId('mermaid-diagram').locator('svg')).toBeVisible();
  await expect(answer.locator('pre code.hljs')).toHaveCount(1);
  await expect(answer.locator('table')).toBeVisible();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
