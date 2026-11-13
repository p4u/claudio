import { test, expect } from '@playwright/test';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { startServer } from '../fixtures/server.mjs';

// The rate-limit test spends this peer's minute: it stays last.
let srv;
test.beforeAll(async () => { srv = await startServer(); });
test.afterAll(async () => { await srv?.stop(); });

/** A ticket as `claudio` mints one (docs/web-protocol.md, "Sign-in tickets"), its file `age` s old. */
function mint(age = 0) {
  const ticket = crypto.randomBytes(32).toString('hex');
  const dir = path.join(srv.root, 'run/claudio/web-tickets');
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  fs.chmodSync(dir, 0o700);
  const file = path.join(dir, crypto.createHash('sha256').update(ticket).digest('hex'));
  fs.writeFileSync(file, '', { mode: 0o600, flag: 'wx' });
  if (age) { const t = Date.now() / 1000 - age; fs.utimesSync(file, t, t); }
  return { ticket, file };
}

const EXPIRED = 'This sign-in link has expired. Run claudio web again.';
const csp = (page) => page.evaluate(() => window.__claudioCsp ?? []);
const hash = (page) => page.evaluate(() => location.hash);
const cookieIn = async (context) => (await context.cookies()).find((c) => c.name === 'claudio_web')?.value;

test('a ticket link signs in once and clears the fragment', async ({ page, browser }) => {
  const { ticket, file } = mint();
  await page.goto(`${srv.origin}/#k=${ticket}`);
  await expect(page.getByTestId('rail')).toBeVisible();
  expect(await hash(page)).toBe('');
  expect(fs.existsSync(file)).toBe(false);
  expect(await cookieIn(page.context())).toMatch(/^[0-9a-f]{64}$/);
  expect(await csp(page)).toEqual([]);
  const other = await browser.newContext();
  const replay = await other.newPage();
  await replay.goto(`${srv.origin}/#k=${ticket}`);
  await expect(replay.getByText(EXPIRED)).toBeVisible();
  expect(await hash(replay)).toBe('');
  expect(await cookieIn(other)).toBeUndefined();
  expect(await csp(replay)).toEqual([]);
  await other.close();
});

test('an expired or malformed ticket shows the expiry message', async ({ page }) => {
  const posted = [];
  page.on('request', (r) => { if (r.url().endsWith('/api/login')) posted.push(r.url()); });
  const { ticket, file } = mint(60);
  await page.goto(`${srv.origin}/#k=${ticket}`);
  await expect(page.getByText(EXPIRED)).toBeVisible();
  expect(await hash(page)).toBe('');
  expect(fs.existsSync(file)).toBe(false);
  expect(posted).toHaveLength(1);
  await page.goto('about:blank');
  await page.goto(`${srv.origin}/#k=${ticket.toUpperCase()}`);
  await expect(page.getByText(EXPIRED)).toBeVisible();
  expect(await hash(page)).toBe('');
  expect(posted).toHaveLength(1);
  expect(await csp(page)).toEqual([]);
});

test('a ticket opened with a live cookie replaces that sign-in', async ({ page, request }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  const old = await cookieIn(page.context());
  const { ticket, file } = mint();
  await page.goto('about:blank');
  await page.goto(`${srv.origin}/#k=${ticket}`);
  await expect(page.getByTestId('rail')).toBeVisible();
  expect(fs.existsSync(file)).toBe(false);
  const now = await cookieIn(page.context());
  expect(now).toMatch(/^[0-9a-f]{64}$/);
  expect(now).not.toBe(old);
  const me = (value) => request.get(`${srv.origin}/api/me`, { headers: { Cookie: `claudio_web=${value}` } });
  expect((await me(old)).status()).toBe(401);
  expect((await me(now)).status()).toBe(204);
  expect(await csp(page)).toEqual([]);
});

test('a ticket refused by the rate limit shows the wait message', async ({ page, request }) => {
  const wrong = () => request.post(`${srv.origin}/api/login`, {
    data: { ticket: crypto.randomBytes(32).toString('hex') }, headers: { Origin: srv.origin },
  });
  let status = 0;
  for (let i = 0; i < 6 && status !== 429; i++) status = (await wrong()).status();
  expect(status).toBe(429);
  const { ticket, file } = mint();
  await page.goto(`${srv.origin}/#k=${ticket}`);
  await expect(page.getByText('Too many attempts: wait a minute.')).toBeVisible();
  expect(await hash(page)).toBe('');
  expect(fs.existsSync(file)).toBe(true);
  expect(await csp(page)).toEqual([]);
});
