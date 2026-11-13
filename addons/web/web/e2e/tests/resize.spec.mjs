import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

// How pane 0's terminal sits in its box: the grid's rows and height, the box's height, and
// whether the cursor row (where claude's prompt is) is inside the visible box.
async function fit(page) {
  return page.evaluate(() => {
    const box = document.querySelector('[data-testid="pane"][data-pane="0"] [data-testid="terminal"]');
    const screen = box.querySelector('.xterm-screen');
    const rows = box.querySelectorAll('.xterm-rows > div').length;
    const cursor = box.querySelector('.xterm-helper-textarea');
    const b = box.getBoundingClientRect(), s = screen.getBoundingClientRect(), c = cursor.getBoundingClientRect();
    const cell = rows ? s.height / rows : 0;
    return {
      rows, boxH: b.height, screenH: s.height,
      fits: s.height <= b.height + 1 && s.height > b.height - 2 * cell - 10,
      cursorInside: c.top >= b.top - 1 && c.top + cell <= b.bottom + 1,
    };
  });
}

async function open(page, lines = 50) {
  await page.setViewportSize({ width: 1200, height: 800 });
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const term = page.locator('[data-testid="pane"][data-pane="0"]').getByTestId('terminal');
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  await term.click();
  for (let i = 1; i <= lines; i++) { await page.keyboard.type(`x${i}`); await page.keyboard.press('Enter'); }
  if (lines) await expect(term).toContainText(`line:x${lines}`);
  return { sid, term };
}

const settled = (page, check) => expect.poll(async () => check(await fit(page)), { timeout: 5_000 }).toBe(true);

test('the grid follows window resizes and screen density changes, prompt in view', async ({ page }) => {
  await open(page);
  await settled(page, (f) => f.fits && f.cursorInside);
  for (const [w, h] of [[960, 520], [1500, 950], [1000, 460]]) {
    await page.setViewportSize({ width: w, height: h });
    await settled(page, (f) => f.fits && f.cursorInside);
  }
  const cdp = await page.context().newCDPSession(page);
  for (const dpr of [2, 1.25]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 700, deviceScaleFactor: dpr, mobile: false });
    await settled(page, (f) => f.fits && f.cursorInside);
  }
});

test('another client sizing the terminal never hides the prompt, and typing here takes the size back', async ({ page }) => {
  const { sid, term } = await open(page);
  await settled(page, (f) => f.fits && f.cursorInside);
  // Another client (a terminal UI elsewhere) makes the PTY taller than this pane.
  await srv.api('session.attach', { sid, rows: 70, cols: 140 });
  await srv.api('session.resize', { sid, rows: 70, cols: 140 });
  await expect.poll(async () => (await fit(page)).rows, { timeout: 5_000 }).toBe(70);
  await settled(page, (f) => f.cursorInside);
  // With no key, paste or click here, the size stays the other client's: the pane never trades
  // it on its own (terminal replies, snapshots).
  await page.waitForTimeout(1_000);
  expect((await fit(page)).rows).toBe(70);
  // Typing in this pane takes the size back.
  await term.click();
  await page.keyboard.type('mine');
  await page.keyboard.press('Enter');
  await expect(term).toContainText('line:mine');
  await settled(page, (f) => f.fits && f.cursorInside);
});

test('a prompt near the top stays in view under a taller foreign size', async ({ page }) => {
  const { sid } = await open(page, 0);
  await settled(page, (f) => f.fits && f.cursorInside);
  await srv.api('session.attach', { sid, rows: 80, cols: 140 });
  await srv.api('session.resize', { sid, rows: 80, cols: 140 });
  await expect.poll(async () => (await fit(page)).rows, { timeout: 5_000 }).toBe(80);
  await settled(page, (f) => f.cursorInside);
});

// The vertical scrollbar against the grid: its opacity, and whether it covers the last column.
const scrollbar = (page) => page.evaluate(() => {
  const host = document.querySelector('[data-testid="pane"][data-pane="0"] [data-testid="terminal"]');
  const bar = host.querySelector('.xterm-scrollable-element > .scrollbar.vertical');
  const row = host.querySelector('.xterm-rows > div').getBoundingClientRect();
  const b = bar.getBoundingClientRect(), h = host.getBoundingClientRect();
  return { opacity: getComputedStyle(bar).opacity, clear: b.left >= row.right - 0.5, inside: b.right <= h.right + 0.5 };
});

test('the scrollbar: hidden with nothing to scroll, shown with scrollback, never over the text', async ({ page }) => {
  await open(page, 0);
  await page.mouse.move(0, 0);
  await page.waitForTimeout(1_200);
  expect((await scrollbar(page)).opacity).toBe('0');
  await page.locator('[data-testid="pane"][data-pane="0"]').getByTestId('terminal').click();
  for (let i = 1; i <= 50; i++) { await page.keyboard.type(`x${i}`); await page.keyboard.press('Enter'); }
  // Away from the terminal and past xterm's 800 ms fade: still shown, right of the last column.
  await page.mouse.move(0, 0);
  await page.waitForTimeout(1_500);
  expect(await scrollbar(page)).toEqual({ opacity: '1', clear: true, inside: true });
});
