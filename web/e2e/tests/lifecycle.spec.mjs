import fs from 'node:fs';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { writeTranscript, FIXTURE_WORD } from '../fixtures/transcript.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => { await srv.reset(); });

const card = (page, sid) => page.locator(`[data-testid="session-card"][data-sid="${sid}"]`);
const pane = (page, i) => page.locator(`[data-testid="pane"][data-pane="${i}"]`);

async function openIn(page, sid) {
  await page.goto(srv.url);
  await card(page, sid).click();
  await expect(pane(page, 0).getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
}

test('a terminal tab runs echo ok', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await openIn(page, sid);
  await pane(page, 0).getByTestId('terminal-tab-btn').click();
  const shell = pane(page, 1).getByTestId('terminal');
  await expect(shell).toBeVisible();
  await shell.click();
  await page.keyboard.type('echo ok');
  await page.keyboard.press('Enter');
  await expect.poll(async () => (await shell.innerText()).split('\n').map((l) => l.trim())).toContain('ok');
});

test('rename, kill and restart a session', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'SessionStart', { session_id: 'conv-restart', source: 'startup' });
  await openIn(page, sid);
  const p = pane(page, 0);

  await p.getByTestId('rename-btn').click();
  await page.getByTestId('rename-input').fill('renamed-one');
  await page.getByTestId('confirm-yes').click();
  await expect(card(page, sid)).toContainText('renamed-one');

  const nonce = async () => /nonce=(\d+)/.exec(await p.getByTestId('terminal').innerText())?.[1];
  const first = await nonce();
  expect(first).toBeTruthy();
  await p.getByTestId('restart-resume-btn').click();
  await page.getByTestId('confirm-yes').click();
  await expect.poll(async () => { const n = await nonce(); return n && n !== first; }).toBeTruthy();

  await p.getByTestId('kill-btn').click();
  await page.getByTestId('confirm-yes').click();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
});

test('search finds a fixture conversation', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.b, 'conv-search');
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('/');
  await page.getByTestId('search-input').fill(FIXTURE_WORD);
  await expect(page.getByTestId('search-hit').first()).toBeVisible();
});

test('save and insert a snippet', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'Stop', {});
  await openIn(page, sid);
  const composer = pane(page, 0).getByTestId('composer');
  await expect(composer).toBeEnabled();
  await composer.fill('review the diff carefully');
  await page.keyboard.press('Control+k');
  await page.getByTestId('palette-input').fill('Insert snippet');
  await page.getByTestId('palette-item').first().click();
  await expect(page.getByTestId('snippets')).toBeVisible();
  await page.getByTestId('snippet-save').click();
  const snip = page.getByTestId('snippet').filter({ hasText: 'review the diff carefully' });
  await expect(snip).toHaveCount(1);
  await composer.fill('');
  await snip.click();
  await expect(composer).toHaveValue('review the diff carefully');
});

test('toggling an extension rewrites its config.toml line', async ({ page }) => {
  await page.goto(srv.url);
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByTestId('open-extensions').click();
  const toggle = page.locator('[data-testid="extension-row"][data-name="diagram"]').getByTestId('extension-toggle');
  await expect(toggle).toBeVisible();
  await toggle.uncheck();
  await srv.waitFor(() => /^diagram = false$/m.test(fs.readFileSync(srv.configToml, 'utf8')), 'diagram = false');
  await toggle.check();
  await srv.waitFor(() => /^diagram = true$/m.test(fs.readFileSync(srv.configToml, 'utf8')), 'diagram = true');
});

test('claude update shows progress and completion', async ({ page }) => {
  await page.goto(srv.url);
  await page.locator('[data-testid="host-chip"][data-host="local"]').click();
  await page.getByTestId('host-claude-btn').click();
  await expect(page.getByTestId('claude-toast').first()).toHaveAttribute('data-phase', 'updating');
  await expect(page.locator('[data-testid="claude-toast"][data-phase="done"]')).toContainText('0.0.1-fake');
});
