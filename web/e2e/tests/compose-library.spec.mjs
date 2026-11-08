import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

const SHOTS = path.join(os.homedir(), '.cache/claudio/web-ui/shots/compose-lib');
const card = (page, sid) => page.locator(`[data-testid="session-card"][data-sid="${sid}"]`);
const file = (name) => path.join(srv.root, 'cfg/claudio/snippets', `${name}.md`);

async function open(page, sid) {
  await card(page, sid).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  await page.getByTestId('compose-btn').click();
  await expect(page.getByTestId('compose')).toBeVisible();
}

test('drafts survive a reload; prompts are saved by name and loaded in another session; Recent keeps what was sent', async ({ page }) => {
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  const a = await srv.spawn(srv.dirs.a);
  const b = await srv.spawn(srv.dirs.b);
  await srv.fireHook(srv.dirs.a, 'Stop', {});
  await srv.fireHook(srv.dirs.b, 'Stop', {});
  await page.goto(srv.url);
  await open(page, a);

  // autosave and recovery
  await page.getByTestId('composer').fill('Write the release notes\nfor version 2');
  await expect(page.getByTestId('compose-saved')).toContainText(/saved \d+s ago/);
  await page.reload();
  await open(page, a);
  await expect(page.getByTestId('composer')).toHaveValue('Write the release notes\nfor version 2');

  // Save as... (the name is pre-filled from the first line)
  await page.getByTestId('compose-save-as').click();
  await expect(page.getByTestId('save-as-input')).toHaveValue('Write the release notes');
  await page.getByTestId('save-as-input').fill('release-notes');
  await page.getByTestId('confirm-yes').click();
  const item = page.locator('[data-testid="lib-item"][data-name="release-notes"]');
  await expect(item).toBeVisible();
  expect(fs.readFileSync(file('release-notes'), 'utf8')).toBe('Write the release notes\nfor version 2');
  // saving the same name again asks before replacing
  await page.getByTestId('compose-save-as').click();
  await page.getByTestId('save-as-input').fill('Release Notes');
  await page.getByTestId('confirm-yes').click();
  await expect(page.getByTestId('confirm-yes')).toHaveCount(0); // same text: replaced without a question
  await page.getByTestId('composer').fill('changed text');
  await page.getByTestId('compose-save-as').click();
  await page.getByTestId('save-as-input').fill('release notes');
  await page.getByTestId('confirm-yes').click();
  await expect(page.getByRole('dialog', { name: 'Replace the saved prompt?' })).toBeVisible();
  await page.getByTestId('confirm-no').click();
  expect(fs.readFileSync(file('release-notes'), 'utf8')).toBe('Write the release notes\nfor version 2');
  await expect(page.getByTestId('compose-saved')).toContainText('saved');
  await page.screenshot({ path: path.join(SHOTS, 'compose-saved.png') });
  await page.keyboard.press('Escape');

  // another session: Load, then Insert reaches that session
  await expect(page.getByTestId('compose')).toHaveCount(0);
  await open(page, b);
  await expect(page.getByTestId('composer')).toHaveValue('');
  await page.locator('[data-testid="lib-item"][data-name="release-notes"]').getByTestId('lib-load').click();
  await expect(page.getByTestId('composer')).toHaveValue('Write the release notes\nfor version 2');
  await page.getByTestId('compose-insert').click();
  await srv.waitFor(() => srv.inputLog(srv.dirs.b).includes('Write the release notes'), 'the loaded prompt reached session b');

  // Send: the draft lands in Recent and loads back
  await page.getByTestId('compose-btn').click();
  await page.getByTestId('composer').fill('please summarise the diff');
  await page.keyboard.press('Control+Enter');
  await srv.waitFor(() => srv.inputLog(srv.dirs.b).includes('please summarise the diff'), 'the draft was sent');
  await page.getByTestId('compose-btn').click();
  await expect(page.getByTestId('composer')).toHaveValue('');
  await page.getByTestId('lib-tab-recent').click();
  const recent = page.getByTestId('recent-item');
  await expect(recent.first()).toContainText('please summarise the diff');
  await expect(recent.first()).toContainText('b');
  await page.screenshot({ path: path.join(SHOTS, 'compose-recent.png') });
  await recent.first().getByTestId('recent-load').click();
  await expect(page.getByTestId('composer')).toHaveValue('please summarise the diff');
  // a load keeps the draft it replaced: load the saved prompt over it
  await page.getByTestId('lib-tab-saved').click();
  await page.locator('[data-testid="lib-item"][data-name="release-notes"]').getByTestId('lib-append').click();
  await expect(page.getByTestId('composer')).toHaveValue('please summarise the diff\nWrite the release notes\nfor version 2');
  await page.getByTestId('lib-tab-recent').click();
  await page.getByTestId('recent-remove').first().click();

  // Rename, then Delete removes the file
  await page.getByTestId('lib-tab-saved').click();
  await page.locator('[data-testid="lib-item"][data-name="release-notes"]').getByTestId('lib-rename').click();
  await page.getByTestId('rename-prompt-input').fill('notes v2');
  await page.getByTestId('confirm-yes').click();
  await expect(page.locator('[data-testid="lib-item"][data-name="notes-v2"]')).toBeVisible();
  await expect(page.locator('[data-testid="lib-item"][data-name="release-notes"]')).toHaveCount(0);
  expect(fs.existsSync(file('release-notes'))).toBe(false);
  await page.locator('[data-testid="lib-item"][data-name="notes-v2"]').getByTestId('lib-delete').click();
  await page.getByTestId('confirm-yes').click();
  await expect(page.getByTestId('lib-item')).toHaveCount(0);
  expect(fs.existsSync(file('notes-v2'))).toBe(false);
});
