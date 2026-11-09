import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('a pane shows the terminal and typed keys echo', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const pane = page.locator('[data-testid="pane"][data-pane="0"]');
  const term = pane.getByTestId('terminal');
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  await term.click();
  await page.keyboard.type('hello');
  await page.keyboard.press('Enter');
  await expect(term).toContainText('line:hello');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === 'hello\n', 'hello in the input log');
});
