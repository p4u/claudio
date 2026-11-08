import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

const SHOTS = path.join(os.homedir(), '.cache/claudio/web-ui/shots/compose');
const pane = (page, i) => page.locator(`[data-testid="pane"][data-pane="${i}"]`);

async function enter(page, sid) {
  await srv.fireHook(srv.dirs.a, 'Stop', {});
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  const term = pane(page, 0).getByTestId('terminal');
  await expect(term).toContainText('FAKE_CLAUDE_BANNER');
  await term.click();
  await expect(page.getByTestId('mode-badge')).toHaveText('TERM');
  return term;
}

test('a claude pane has no inline composer; Alt+O opens Compose', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await expect(page.getByTestId('composer')).toHaveCount(0);
  await expect(page.getByTestId('composer-inline')).toHaveCount(0);
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: path.join(SHOTS, 'pane-no-composer.png') });
  await page.keyboard.press('Alt+o');
  await expect(page.getByTestId('compose')).toBeVisible();
  await expect(page.getByTestId('composer')).toBeFocused();
  await page.getByTestId('composer').fill('first line\nsecond line');
  await page.screenshot({ path: path.join(SHOTS, 'compose-dialog.png') });
  // Esc closes and keeps the draft
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('compose')).toHaveCount(0);
  await page.getByTestId('compose-btn').click();
  await expect(page.getByTestId('composer')).toHaveValue('first line\nsecond line');
});

test('Insert pastes without submitting, Send submits, focus returns to the terminal', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  const term = await enter(page, sid);
  await page.keyboard.press('Alt+o');
  await page.getByTestId('composer').fill('alpha\nbeta');
  await page.getByTestId('compose-insert').click();
  await expect(page.getByTestId('compose')).toHaveCount(0);
  await expect(page.getByTestId('mode-badge')).toHaveText('TERM');
  // the paste reached claude's input (cooked in the fake: the first line is complete, the last one waits for Enter)
  await srv.waitFor(() => srv.inputLog(srv.dirs.a).includes('alpha'), 'the first pasted line in the input log');
  expect(srv.inputLog(srv.dirs.a)).not.toContain('beta');
  await expect(term).toContainText('beta');
  // the text can still be edited, then claude's own Enter submits it
  await page.keyboard.press('Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a).includes('beta'), 'beta submitted by claude\'s own Enter');

  await page.keyboard.press('Alt+o');
  await expect(page.getByTestId('composer')).toHaveValue('');
  await page.getByTestId('composer').fill('gamma');
  await page.keyboard.press('Control+Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a).includes('gamma'), 'gamma sent');
  await expect(page.getByTestId('mode-badge')).toHaveText('TERM');
  await page.getByTestId('compose-btn').click();
  await page.getByTestId('composer').fill('delta');
  await page.getByTestId('compose-send').click();
  await srv.waitFor(() => srv.inputLog(srv.dirs.a).includes('delta'), 'delta sent');
});

test('a prompt over 64 KiB is refused with a message', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.keyboard.press('Alt+o');
  await page.getByTestId('composer').fill('x'.repeat(64 * 1024 + 1));
  await expect(page.getByTestId('compose-over')).toContainText('64 KiB');
  await expect(page.getByTestId('compose-send')).toBeDisabled();
});

test('the setting brings the inline composer back', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.keyboard.press('Alt+z');
  await page.getByTestId('palette-input').fill('Settings');
  await page.getByTestId('palette-item').first().click();
  await page.getByTestId('inline-composer').check();
  await page.keyboard.press('Escape');
  const inline = pane(page, 0).getByTestId('composer-inline');
  await expect(inline).toBeVisible();
  await inline.fill('inline prompt');
  await page.keyboard.press('Control+Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.a).includes('inline prompt'), 'inline prompt sent');
});

test('Alt+Shift+F fills the window and again restores it; the button does the same', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await enter(page, sid);
  await page.setViewportSize({ width: 1440, height: 900 });
  const box = async () => pane(page, 0).boundingBox();
  const before = await box();
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+Shift+F');
  await expect(page.getByTestId('rail')).toBeHidden();
  await expect(page.getByTestId('fullscreen-exit')).toBeVisible();
  await expect.poll(async () => { const b = await box(); return [b.x, b.y, b.width, b.height]; }).toEqual([0, 0, 1440, 900]);
  await expect(page.getByTestId('mode-badge')).toBeHidden(); // the top bar is gone too
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: path.join(SHOTS, 'pane-maximized.png') });
  // the terminal still has the banner (nothing was unmounted) and still takes keys
  await expect(pane(page, 0).getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  await page.keyboard.press('Alt+Shift+F');
  await expect(page.getByTestId('rail')).toBeVisible();
  await expect.poll(async () => JSON.stringify(await box())).toBe(JSON.stringify(before));
  await pane(page, 0).getByTestId('fullscreen-btn').click();
  await expect(page.getByTestId('rail')).toBeHidden();
  await page.getByTestId('fullscreen-exit').click();
  await expect(page.getByTestId('rail')).toBeVisible();
  // Esc is not bound: it reaches claude
  await pane(page, 0).getByTestId('terminal').click();
  await page.keyboard.press('Alt+Shift+F');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('fullscreen-exit')).toBeVisible();
});
