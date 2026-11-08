import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

const card = (page, sid) => page.locator(`[data-testid="session-card"][data-sid="${sid}"]`);
const SHOTS = path.join(os.homedir(), '.cache/claudio/web-ui/shots/compose-lib');

test('the rail marks the session of the focused pane, and the ones on screen', async ({ page }) => {
  const a = await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(srv.url);
  await card(page, a).click();
  await expect(card(page, a)).toHaveAttribute('aria-current', 'true');
  await card(page, b).click();
  await expect(card(page, b)).toHaveAttribute('aria-current', 'true');
  await expect(card(page, a)).not.toHaveAttribute('aria-current', 'true');
  await page.mouse.move(700, 450);
  await page.waitForTimeout(300);
  const style = (el) => el.evaluate((e) => { const c = getComputedStyle(e); return { img: c.backgroundImage, bg: c.backgroundColor, outline: c.outlineStyle, ow: c.outlineWidth, h: e.getBoundingClientRect().height }; });
  const picked = await style(card(page, b));
  expect(picked.img).toContain('linear-gradient');
  expect(picked.outline).toBe('solid');
  expect(picked.ow).toBe('1px');
  expect(picked.bg).toBe('rgb(31, 33, 36)'); // Graphite
  const other = await style(card(page, a));
  expect(other.img).toBe('none');
  expect(other.h).toBe(picked.h); // rows keep their height
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.locator('[data-testid="rail"]').screenshot({ path: path.join(SHOTS, 'rail-picked.png') });
});
