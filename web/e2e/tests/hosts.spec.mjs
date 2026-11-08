import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

test('the Hosts button opens a menu of hosts, keyboard and outside click close it', async ({ page }) => {
  await page.goto(srv.url);
  const btn = page.getByTestId('hosts-btn');
  await expect(btn).toContainText('Hosts');
  await expect(page.getByTestId('hosts-summary')).toHaveText('1/1');
  await expect(page.getByTestId('hosts-menu')).toHaveCount(0);
  const before = await page.locator('.topbar').boundingBox();
  await btn.focus();
  await page.keyboard.press('Enter');
  const menu = page.getByTestId('hosts-menu');
  await expect(menu).toBeVisible();
  expect((await page.locator('.topbar').boundingBox()).height).toBe(before.height);
  await expect(menu.locator('[data-testid="host-chip"][data-host="local"]')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByTestId('host-add-chip')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(btn).toBeFocused();
  await btn.click();
  await expect(menu).toBeVisible();
  await page.mouse.click(700, 500);
  await expect(menu).toHaveCount(0);
  await btn.click();
  await menu.getByTestId('host-add-chip').click();
  await expect(menu).toHaveCount(0);
  await expect(page.getByTestId('hosts-panel').or(page.getByRole('dialog')).first()).toBeVisible();
});

test('a host row shows the whole name on one line, the menu widens to fit', async ({ page }) => {
  const long = 'build-server-eu-west';
  const s2 = await startServer({ hosts: ['z6', long] });
  try {
    await page.goto(s2.url);
    await page.getByTestId('hosts-btn').click();
    const menu = page.getByTestId('hosts-menu');
    // A connected host also has bars, a long claude version and badges: the name must keep its width.
    await menu.locator('.hosts-row[data-host]').evaluateAll((rows) => rows.forEach((r) => {
      r.querySelector('.hosts-ver').textContent = '2.1.123 (Claude Code) build abcdef0123456789 extra text';
      for (let i = 0; i < 2; i++) r.insertAdjacentHTML('beforeend', '<span class="chip-badge">↑</span>');
    }));
    for (const name of ['local', 'z6', long]) {
      const row = menu.locator(`[data-testid="host-chip"][data-host="${name}"]`);
      await expect(row).toBeVisible();
      await expect(row.locator('.hosts-name')).toHaveText(name);
      const box = await row.locator('.hosts-name').evaluate((el) => ({ w: el.getBoundingClientRect().width, clipped: el.scrollWidth > el.clientWidth, h: el.getBoundingClientRect().height }));
      expect(box.w).toBeGreaterThan(name.length * 5);
      expect(box.clipped, `${name} is cut`).toBe(false);
      expect(box.h).toBeLessThan(30); // one line
    }
    await menu.screenshot({ path: process.env.HOME + '/.cache/claudio/web-ui/shots/compose-lib/hosts-menu.png' });
    const w = (await menu.boundingBox()).width;
    expect(w).toBeGreaterThanOrEqual(280);
    expect(w).toBeLessThanOrEqual(520);
  } finally { await s2.stop(); }
});
