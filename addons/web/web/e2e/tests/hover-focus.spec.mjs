import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

async function open(page) {
  await page.setViewportSize({ width: 1200, height: 800 });
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const term = page.locator('[data-testid="pane"][data-pane="0"]').getByTestId('terminal');
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  return { term, badge: page.getByTestId('mode-badge'), rail: page.getByTestId('rail') };
}

test('the keyboard follows the mouse: the terminal under it, the page elsewhere', async ({ page }) => {
  const { term, badge, rail } = await open(page);
  await rail.hover();
  await expect(badge).toHaveAttribute('data-mode', 'app');
  await term.hover();
  await expect(badge).toHaveAttribute('data-mode', 'terminal');
  await page.keyboard.type('hovered');
  await page.keyboard.press('Enter');
  await expect(term).toContainText('line:hovered');
  // Back over the rail: single keys drive the page again (k opens the palette).
  await rail.hover();
  await expect(badge).toHaveAttribute('data-mode', 'app');
  await page.keyboard.press('k');
  await expect(page.getByTestId('palette')).toBeVisible();
});

test('the mouse never takes the keyboard from the palette or a text field', async ({ page }) => {
  const { term, badge, rail } = await open(page);
  // The palette and the search cover the terminal: move the mouse to where it is.
  const b = await term.boundingBox();
  const overTerm = () => page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 4 });
  await rail.hover();
  await page.keyboard.press('k');
  const palette = page.getByTestId('palette');
  await expect(palette).toBeVisible();
  await overTerm();
  await page.keyboard.type('xyz');
  await expect(palette.locator('input')).toHaveValue('xyz');
  await page.keyboard.press('Escape');
  await expect(palette).toBeHidden();
  // The search field keeps the keyboard while the mouse is over the terminal.
  await rail.hover();
  await page.keyboard.press('/');
  const search = page.getByTestId('search-input');
  await expect(search).toBeFocused();
  await overTerm();
  await page.keyboard.type('abc');
  await expect(search).toHaveValue('abc');
  await expect(badge).toHaveAttribute('data-mode', 'app');
});

test('an open menu keeps the keyboard while the mouse crosses a terminal', async ({ page }) => {
  const { term, badge } = await open(page);
  const b = await term.boundingBox();
  await page.getByTestId('hosts-btn').click();
  const menu = page.getByTestId('hosts-menu');
  await expect(menu).toBeVisible();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 4 });
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(badge).toHaveAttribute('data-mode', 'app');
  expect(srv.inputLog(srv.dirs.a)).toBe('');
});
