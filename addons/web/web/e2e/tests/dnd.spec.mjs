import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

const card = (page, sid) => page.locator(`[data-testid="session-card"][data-sid="${sid}"]`);
const pane = (page, i) => page.locator(`[data-testid="pane"][data-pane="${i}"]`);
const order = (page) => page.locator('[data-testid="session-card"]').evaluateAll((els) => els.map((e) => e.dataset.sid));

test('dragging a rail card onto a pane opens it there, even after the hover info showed', async ({ page }) => {
  const a = await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await page.goto(srv.url);
  await page.getByTestId('grid-btn').and(page.locator('[data-grid="2"]')).click();
  await expect(page.getByTestId('pane')).toHaveCount(2);
  await card(page, a).dragTo(pane(page, 0));
  await expect(pane(page, 0).locator('.pane-title')).toHaveText('a');
  // hover long enough for the info popup, then drag
  await card(page, b).hover();
  await expect(page.getByTestId('session-info')).toBeVisible();
  await page.waitForTimeout(300);
  await card(page, b).dragTo(pane(page, 1));
  await expect(pane(page, 1).locator('.pane-title')).toHaveText('b');
  await expect(page.getByTestId('session-info')).toHaveCount(0);
});

test('dragging a card onto another reorders the rail and the order persists', async ({ page }) => {
  const a = await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await page.goto(srv.url);
  await expect(page.getByTestId('session-card')).toHaveCount(2);
  const before = await order(page);
  expect(before.sort()).toEqual([a, b].sort());
  const [first, second] = await order(page);
  // drop the second card on the upper half of the first
  await card(page, second).dragTo(card(page, first), { targetPosition: { x: 20, y: 3 } });
  await expect.poll(() => order(page)).toEqual([second, first]);
  await page.reload();
  await expect(page.getByTestId('session-card')).toHaveCount(2);
  expect(await order(page)).toEqual([second, first]);
});

test('a board row can be dragged onto a pane', async ({ page }) => {
  const a = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await expect(page.getByTestId('board')).toBeVisible();
  await page.locator(`[data-testid="board-row"][data-sid="${a}"]`).dragTo(page.getByTestId('board'));
  await expect(page.getByTestId('pane')).toHaveCount(1);
  await expect(pane(page, 0).locator('.pane-title')).toHaveText('a');
});
