import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

async function enter(page, sid) {
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const term = page.getByTestId('terminal').first();
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  await term.click();
  await expect(page.getByTestId('mode-badge')).toHaveText('TERM');
  return term;
}

test('Alt+u in a terminal opens the reader and sends nothing to claude', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.keyboard.press('Alt+u');
  await expect(page.getByTestId('drawer')).toBeVisible();
  await page.keyboard.press('Alt+u');
  await expect(page.getByTestId('drawer')).toHaveCount(0);
  // a marker line proves the earlier keys did not land in the input before it
  await page.getByTestId('terminal').first().click();
  await page.keyboard.type('mark');
  await page.keyboard.press('Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === 'mark\n', 'only the marker in the input log');
});

test('Alt+a focuses the session that needs attention, Alt+Right switches session', async ({ page }) => {
  await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await enter(page, b);
  const shown = async () => page.locator('[data-testid="pane"][data-pane="0"] .pane-title').innerText();
  const labelB = await shown();
  await page.keyboard.press('Alt+ArrowRight');
  await expect.poll(shown).not.toBe(labelB);
  await srv.fireHook(srv.dirs.b, 'Notification', { notification_type: 'permission_prompt', message: 'needs you' });
  // the hook's state reaches the page a moment later: press until the queue has the session
  await expect(async () => {
    await page.keyboard.press('Alt+a');
    expect(await shown()).toBe(labelB);
  }).toPass({ timeout: 10_000 });
});

test('Shift+Enter puts ESC CR into the input, Enter submits', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.keyboard.type('a');
  await page.keyboard.press('Shift+Enter');
  await page.keyboard.type('b');
  await page.keyboard.press('Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === 'a\x1b\nb\n', 'a, ESC, newline, b');
});

test('the help has the key check and describes keys without sending them', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.keyboard.press('Alt+h');
  await expect(page.getByTestId('help')).toBeVisible();
  await expect(page.getByTestId('key-check')).toBeVisible();
  await page.getByTestId('key-check-toggle').click();
  await page.keyboard.press('Alt+u');
  await expect(page.getByTestId('key-check-out')).toContainText('claudio: reader');
  await page.keyboard.press('Control+c');
  await expect(page.getByTestId('key-check-out')).toContainText('sent to the session: \\x03');
  await expect(page.getByTestId('drawer')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.getByTestId('terminal').first().click();
  await page.keyboard.type('z');
  await page.keyboard.press('Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a) === 'z\n', 'nothing was sent by the key check');
});

test('the settings have Capture all keys and the manifest is served', async ({ page }) => {
  await page.goto(srv.url);
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByTestId('capture-keys')).toBeVisible();
  const r = await page.request.get(new URL('/manifest.webmanifest', srv.url).href);
  expect(r.headers()['content-type']).toContain('application/manifest+json');
  expect((await r.json()).display).toBe('standalone');
});
