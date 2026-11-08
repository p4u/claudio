import { defineConfig } from '@playwright/test';

// Fixtures start one real `claudio web` per spec file (beforeAll); no webServer here.
export default defineConfig({
  testDir: 'tests',
  workers: 1,
  fullyParallel: false,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: 'line',
  use: {
    browserName: 'chromium',
    headless: true,
    viewport: { width: 1600, height: 900 },
    actionTimeout: 10_000,
  },
});
