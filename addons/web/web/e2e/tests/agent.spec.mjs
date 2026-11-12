import { test, expect } from '@playwright/test';
import fs from 'node:fs';
import { startServer } from '../fixtures/server.mjs';

// A remembered remote host (offline: the ssh to it just fails) has a host panel.
let srv;
test.beforeAll(async () => { srv = await startServer({ hosts: ['z6'] }); });
test.afterAll(async () => { await srv?.stop(); });

test('the host panel switches ssh agent forwarding for a remote host', async ({ page }) => {
  await page.goto(srv.url);
  await page.getByTestId('hosts-btn').click();
  await page.getByTestId('hosts-menu').locator('[data-testid="host-chip"][data-host="z6"]').click();
  await expect(page.getByTestId('host-panel-facts')).toHaveAttribute('data-host', 'z6');
  const row = page.getByTestId('host-agent');
  const btn = page.getByTestId('host-agent-btn');
  await expect(row).toHaveText('not forwarded (your ssh config decides)');
  await btn.click();
  await expect(row).toHaveText('forward on connect');
  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('forward_agent = ["z6"]');
  await btn.click();
  await expect(row).toHaveText('not forwarded (your ssh config decides)');
  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('forward_agent = []');
  // local has no ssh connection: no row.
  await page.getByRole('tab', { name: 'local' }).click();
  await expect(page.getByTestId('host-panel-facts')).toHaveAttribute('data-host', 'local');
  await expect(page.getByTestId('host-agent')).toHaveCount(0);
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});

test('the wizard sends the agent choice before it connects a typed host', async ({ page }) => {
  const sent = [];
  page.on('websocket', (ws) => ws.on('framesent', (f) => {
    if (typeof f.payload !== 'string') return;
    try { sent.push(JSON.parse(f.payload)); } catch { /* not JSON */ }
  }));
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  await page.getByTestId('wizard-host').selectOption('other:');
  await page.getByTestId('wizard-host-input').fill('nonexistent.invalid');
  const agent = page.getByTestId('wizard-host-agent');
  await expect(agent).not.toBeChecked();
  await agent.check();
  await page.getByTestId('wizard-host-connect').click();
  await expect(page.getByTestId('wizard-host-error')).toContainText(/resolve|ssh failed/i, { timeout: 60_000 });
  const ops = sent.filter((m) => m.t === 'req').map((m) => m.op);
  const fwd = ops.indexOf('hosts.forward_agent');
  expect(fwd).toBeGreaterThanOrEqual(0);
  expect(fwd).toBeLessThan(ops.indexOf('hosts.connect'));
  expect(sent.find((m) => m.op === 'hosts.forward_agent')).toMatchObject({ host: 'nonexistent.invalid', on: true });
  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('"nonexistent.invalid"');
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
