// The wizard's folder combobox (typed navigation from ~, Tab/Backspace/Enter/Esc, hidden
// folders, errors, "Browse folders…") and its "Find a conversation…" field.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { writeTranscript, FIXTURE_WORD } from '../fixtures/transcript.mjs';

let srv;
let home;
test.beforeAll(async () => {
  srv = await startServer();
  home = fs.realpathSync(path.join(srv.root, 'home'));
  for (const d of ['alpha', 'Beta', 'gamma/inner', '.cfg']) fs.mkdirSync(path.join(home, d), { recursive: true });
  fs.writeFileSync(path.join(home, 'afile'), '');
});
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async ({ page }) => {
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
  await srv.reset();
});

async function openWizard(page) {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  await page.getByTestId('wizard-launch').selectOption('direct');
  const input = page.getByTestId('wizard-dir-input');
  await input.focus();
  return input;
}

test('typing ~/ lists the home folders; typing filters, Tab enters, Backspace goes up', async ({ page }) => {
  const input = await openWizard(page);
  const rows = page.getByTestId('wizard-dir-row');
  await input.pressSequentially('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await expect(page.getByTestId('wizard-dir-use')).toHaveText('▸ use ~');
  await expect(page.getByTestId('wizard-dir-use')).toHaveAttribute('aria-selected', 'true');
  await input.press('g');
  await expect(rows).toHaveText(['gamma/']);
  await expect(rows.first()).toHaveAttribute('aria-selected', 'true');
  await input.press('Tab');
  await expect(input).toHaveValue('~/gamma/');
  await expect(input).toBeFocused();
  await expect(rows).toHaveText(['inner/']);
  await expect(rows.first()).toHaveAttribute('data-path', '~/gamma/inner');
  await input.press('Backspace');
  await expect(input).toHaveValue('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  // Esc closes the list and stops there; a second Esc closes the dialog.
  await input.press('Escape');
  await expect(page.getByTestId('wizard-dir-list')).toHaveCount(0);
  await expect(page.getByTestId('wizard')).toBeVisible();
  await input.press('Escape');
  await expect(page.getByTestId('wizard')).toHaveCount(0);
});

test('a path typed before the connection is up is listed once it is', async ({ page }) => {
  // The socket reports open 1.5 s late (the page's requests wait for it).
  await page.addInitScript(() => {
    const W = window.WebSocket;
    window.WebSocket = class extends W {
      set onopen(f) { super.onopen = (e) => setTimeout(() => f(e), 1500); }
      get onopen() { return super.onopen; }
    };
  });
  const input = await openWizard(page);
  await input.pressSequentially('~/');
  await expect(page.getByTestId('wizard-dir-row')).toHaveText(['alpha/', 'Beta/', 'gamma/']);
});

test('hidden folders show with the toggle or a dot fragment', async ({ page }) => {
  const input = await openWizard(page);
  const rows = page.getByTestId('wizard-dir-row');
  await input.pressSequentially('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await expect(page.locator('[data-testid="wizard-dir-row"][data-path="~/.cfg"]')).toHaveCount(0);
  const hidden = page.getByTestId('wizard').getByLabel('hidden', { exact: true });
  await hidden.check();
  await expect(rows).toHaveText(['.cfg/', 'alpha/', 'Beta/', 'gamma/']);
  await hidden.uncheck();
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await input.press('.');
  await expect(rows).toHaveText(['.cfg/']);
});

test('Enter on use picks the folder and focuses Start; Enter again starts there', async ({ page }) => {
  const input = await openWizard(page);
  await input.pressSequentially('~/alpha');
  await expect(page.getByTestId('wizard-dir-row')).toHaveText(['alpha/']);
  await input.press('Enter'); // a folder row completes, starts nothing
  await expect(input).toHaveValue('~/alpha/');
  await expect(page.getByTestId('wizard-dir-use')).toHaveAttribute('aria-selected', 'true');
  await input.press('Enter'); // "use": the folder, the list closes, Start has the focus
  await expect(input).toHaveValue('~/alpha');
  await expect(page.getByTestId('wizard-dir-list')).toHaveCount(0);
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
  await page.keyboard.press('Enter');
  const card = page.getByTestId('session-card');
  await expect(card).toHaveCount(1);
  await card.hover();
  await expect(page.getByTestId('session-info')).toContainText(path.join(home, 'alpha'));
});

test('a missing folder never starts a session in ~', async ({ page }) => {
  const input = await openWizard(page);
  await input.fill('~/missing');
  await input.press('Enter'); // before the listing: "use ~/missing", nothing starts
  await expect(input).toHaveValue('~/missing');
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  // After the listing: the row says so and Enter does nothing.
  await input.press('End');
  await input.press('Backspace');
  await input.press('g');
  await expect(input).toBeFocused(); // typing after "use" keeps the focus in the field
  await expect(page.getByTestId('wizard-dir-use')).toHaveText('▸ use ~/missing (not found)');
  await input.press('Enter');
  await expect(page.getByTestId('wizard-dir-list')).toBeVisible();
  await expect(page.getByTestId('wizard')).toBeVisible();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
});

test('an edit after use cancels a pending move to Start', async ({ page }) => {
  const input = await openWizard(page);
  // "use" on a folder that cannot start (`~user` is not expanded): the move to Start waits.
  await input.fill('~x');
  await input.press('Enter');
  await expect(page.getByTestId('wizard-start')).toBeDisabled();
  // Typing makes it startable; the focus stays in the field, so Enter never starts mid-edit.
  await input.press('Home');
  await input.press('/');
  await expect(page.getByTestId('wizard-start')).toBeEnabled();
  await page.keyboard.press('End'); // the page's keyboard: wherever the focus is now
  await page.keyboard.type('y');
  await expect(input).toHaveValue('/~xy');
  await expect(input).toBeFocused();
});

test('Browse folders opens the list at ~/, and an unreadable folder shows its error', async ({ page }) => {
  const input = await openWizard(page);
  await page.getByTestId('wizard-browse').click();
  await expect(input).toHaveValue('~/');
  await expect(input).toBeFocused();
  await expect(page.getByTestId('wizard-dir-row')).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await input.pressSequentially('afile/x');
  await expect(page.getByTestId('wizard-dir-error')).toContainText('cannot list ~/afile/');
  await expect(page.getByTestId('wizard-dir-error')).toContainText('Not a directory');
  await expect(page.getByTestId('wizard-dir-row')).toHaveCount(0);
});

test('Find a conversation fills the wizard and starts it with resume', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.b, 'conv-find');
  const spawns = [];
  page.on('websocket', (ws) => ws.on('framesent', (f) => {
    try {
      const m = JSON.parse(f.payload);
      if (m.op === 'session.spawn') spawns.push(m);
    } catch { /* a binary frame */ }
  }));
  await openWizard(page);
  const find = page.getByTestId('wizard-find');
  await find.fill(FIXTURE_WORD);
  const rows = page.getByTestId('wizard-find-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-conversation', 'conv-find');
  await expect(rows.first()).toContainText(`local · ${srv.dirs.b}`);
  await find.press('Enter'); // picks the highlighted hit, starts nothing
  await expect(page.getByTestId('wizard-dir-input')).toHaveValue(srv.dirs.b);
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  await expect(page.getByTestId('wizard')).toBeVisible();
  expect(spawns).toEqual([]);
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('session-card')).toHaveCount(1);
  await expect.poll(() => spawns.length).toBe(1);
  expect(spawns[0].resume).toBe('conv-find');
  expect(spawns[0].cwd).toBe(srv.dirs.b);
});

test('a hit whose conversation is live focuses that session and closes the wizard', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.a, 'conv-live');
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'SessionStart', { session_id: 'conv-live', source: 'startup' });
  await openWizard(page);
  await page.getByTestId('wizard-find').fill(FIXTURE_WORD);
  await page.locator('[data-testid="wizard-find-row"][data-conversation="conv-live"]').click();
  await expect(page.getByTestId('wizard')).toHaveCount(0);
  await expect(page.locator(`[data-testid="session-card"][data-sid="${sid}"]`)).toHaveAttribute('aria-current', 'true');
  await expect(page.locator('[data-testid="pane"][data-focus="true"]').getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
});

// The folder list takes room in the dialog: closing it at the mousedown on Start moved Start away
// and lost the click (a tall window, or the terminal form, whose body is not scrolled).
for (const [name, kind, size] of [['claude, tall window', 'claude', { width: 1600, height: 1400 }], ['terminal', 'shell', { width: 1600, height: 900 }]]) {
  test(`one click on Start with the folder list open starts the session (${name})`, async ({ page }) => {
    await page.setViewportSize(size);
    const input = await openWizard(page);
    if (kind === 'shell') await page.getByTestId('wizard-kind-shell').click();
    await input.focus();
    await input.pressSequentially(srv.dirs.a);
    await expect(page.getByTestId('wizard-dir-list')).toBeVisible();
    await expect(page.getByTestId('wizard-dir-use')).toBeVisible();
    await page.getByTestId('wizard-start').click();
    await expect(page.getByTestId('session-card')).toHaveCount(1);
    await expect(page.getByTestId('wizard')).toHaveCount(0);
  });
}
