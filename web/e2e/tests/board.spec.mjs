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
  await expect(card(page, b)).toHaveAttribute('data-class', 'stopped');
  await expect(card(page, b).getByRole('img')).toHaveText('■');
  await expect(card(page, a).getByRole('img')).toHaveAttribute('aria-label', 'working');
  await expect(page.getByTestId('board')).toBeVisible();
  const row = (sid) => page.locator(`[data-testid="board-row"][data-sid="${sid}"]`);
  await expect(row(a)).toHaveAttribute('data-state', 'working');
  await expect(row(b)).toHaveAttribute('data-state', 'idle');
});

const brow = (page, sid) => page.locator(`[data-testid="board-row"][data-sid="${sid}"]`);

test('the rail is a static list: no queue section, no extra lines', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'Claude needs your permission' });
  await expect(card(page, sid)).toHaveAttribute('data-class', 'waiting');
  await expect(page.getByTestId('queue-card')).toHaveCount(0);
  await expect(page.getByTestId('queue')).toHaveCount(0);
  const before = await card(page, sid).boundingBox();
  await card(page, sid).hover();
  await page.mouse.move(0, 0);
  await card(page, sid).focus();
  const after = await card(page, sid).boundingBox();
  expect(after.height).toBe(before.height);
  await expect(card(page, sid)).not.toContainText('permission');
});

test('a permission prompt shows its text and Approve resolves it', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.a, 'PreToolUse', { tool_name: 'Bash', tool_input: { command: 'ls -la' } });
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'Claude needs your permission to use Bash' });
  const q = brow(page, sid);
  await expect(q).toContainText('Claude needs your permission to use Bash');
  await expect(q).toHaveAttribute('data-class', 'waiting');
  const before = srv.inputLog(srv.dirs.a);
  await q.getByTestId('approve-btn').click();
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === `${before}\n`, 'an empty input line');
  await srv.fireHook(srv.dirs.a, 'PreToolUse', { tool_name: 'Bash', tool_input: { command: 'ls' } });
  await expect(q.getByTestId('approve-btn')).toHaveCount(0);
  await expect(q).toHaveAttribute('data-class', 'working');
});

test('an elicitation row waits for you but offers no Approve', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.b);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.b, 'Notification', { notification_type: 'elicitation_dialog', message: 'Pick one' });
  const q = brow(page, sid);
  await expect(q).toContainText('Pick one');
  await expect(q).toHaveAttribute('data-class', 'waiting');
  await expect(q.getByTestId('approve-btn')).toHaveCount(0);
  await expect(q.getByTestId('deny-btn')).toHaveCount(0);
});

test('hovering a rail item for a moment shows its info without moving anything', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await srv.fireHook(srv.dirs.a, 'Notification', { notification_type: 'permission_prompt', message: 'needs Bash' });
  const c = card(page, sid);
  await expect(c).toHaveAttribute('data-class', 'waiting');
  const other = await page.getByTestId('rail').boundingBox();
  await c.hover();
  await expect(page.getByTestId('session-info')).toHaveCount(0);
  await expect(page.getByTestId('session-info')).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId('session-info')).toContainText('needs Bash');
  await expect(page.getByTestId('session-info')).toContainText(srv.dirs.a);
  const pop = await page.getByTestId('session-info').boundingBox();
  const cb = await c.boundingBox();
  expect(pop.x >= cb.x + cb.width || pop.x + pop.width <= cb.x).toBe(true);
  expect(await page.getByTestId('rail').boundingBox()).toEqual(other);
  await page.keyboard.press('Shift');
  await expect(page.getByTestId('session-info')).toHaveCount(0);
});
