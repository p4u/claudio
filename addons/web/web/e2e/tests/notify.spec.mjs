import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

test('a hidden session needing approval notifies once', async ({ page, context }) => {
  await context.grantPermissions(['notifications']);
  await page.addInitScript(() => {
    window.__notes = [];
    window.Notification = class {
      static permission = 'granted';
      static async requestPermission() { return 'granted'; }
      constructor(title, opts) { window.__notes.push({ title, body: opts?.body }); }
      close() {}
    };
  });
  await page.goto(srv.url);
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByLabel(/^Notifications/).check();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('settings')).toHaveCount(0);
  const sid = await srv.spawn(srv.dirs.a); // not in any pane
  await expect(page.locator(`[data-testid="session-card"][data-sid="${sid}"]`)).toBeVisible();
  await srv.fireHook(srv.dirs.a, 'PreToolUse', { tool_name: 'Bash', tool_input: { command: 'ls' } });
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'first ask' });
  await expect.poll(() => page.evaluate(() => window.__notes.length)).toBe(1);
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'second ask' });
  await expect(page.locator(`[data-testid="board-row"][data-sid="${sid}"]`)).toContainText('second ask');
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => window.__notes.length)).toBe(1);
});
