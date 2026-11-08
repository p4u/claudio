import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

const card = (page, sid) => page.locator(`[data-testid="session-card"][data-sid="${sid}"]`);

test('the board lists sessions with their states', async ({ page }) => {
  const a = await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await srv.fireHook(srv.dirs.a, 'UserPromptSubmit', { prompt: 'go' });
  await srv.fireHook(srv.dirs.b, 'UserPromptSubmit', { prompt: 'go' });
  await srv.fireHook(srv.dirs.b, 'Stop', {});
  await page.goto(srv.url);
  await expect(card(page, a)).toHaveAttribute('data-state', 'working');
  await expect(card(page, b)).toHaveAttribute('data-state', 'idle');
  await expect(card(page, b).getByRole('img')).toHaveText('✓');
  await expect(card(page, a).getByRole('img')).toHaveAttribute('aria-label', 'working');
  await expect(page.getByTestId('board')).toBeVisible();
  const row = (sid) => page.locator(`[data-testid="board-row"][data-sid="${sid}"]`);
  await expect(row(a)).toHaveAttribute('data-state', 'working');
  await expect(row(b)).toHaveAttribute('data-state', 'idle');
});

test('a permission prompt shows its text and Approve resolves it', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.a, 'PreToolUse', { tool_name: 'Bash', tool_input: { command: 'ls -la' } });
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'Claude needs your permission to use Bash' });
  const q = page.locator(`[data-testid="queue-card"][data-sid="${sid}"]`);
  await expect(q).toContainText('Claude needs your permission to use Bash');
  await expect(q).toContainText('ls -la');
  const before = srv.inputLog(srv.dirs.a);
  await q.getByTestId('approve-btn').click();
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === `${before}\n`, 'an empty input line');
  await srv.fireHook(srv.dirs.a, 'PreToolUse', { tool_name: 'Bash', tool_input: { command: 'ls' } });
  await expect(q).toHaveCount(0);
});

test('an elicitation card offers Open only', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.b);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.b, 'Notification', { notification_type: 'elicitation_dialog', message: 'Pick one' });
  const q = page.locator(`[data-testid="queue-card"][data-sid="${sid}"]`);
  await expect(q).toContainText('Pick one');
  await expect(q.getByTestId('open-btn')).toBeVisible();
  await expect(q.getByTestId('approve-btn')).toHaveCount(0);
  await expect(q.getByTestId('deny-btn')).toHaveCount(0);
});
