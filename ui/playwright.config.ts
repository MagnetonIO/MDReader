import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright e2e for the MDReader shell. The tests run the real shell + real
 * markdown pipeline in Chromium against the Vite dev server, with the Tauri IPC
 * mocked in-page via `@tauri-apps/api/mocks` (see src/e2e/harness.ts, served by
 * e2e.html). Kept out of the vitest gate — run with `pnpm test:e2e`.
 */
export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL: 'http://localhost:1420',
    trace: 'on-first-retry',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm dev',
    url: 'http://localhost:1420/e2e.html',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
