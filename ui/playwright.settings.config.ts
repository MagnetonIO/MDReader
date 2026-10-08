import { defineConfig } from '@playwright/test';
import base from './playwright.config.js';

/** Settings layout/focus only. The IPC fixture does not prove native operations. */
export default defineConfig({
  ...base,
  testMatch: 'settings.spec.ts',
  globalSetup: undefined,
  fullyParallel: false,
  workers: 1,
  use: { ...base.use, baseURL: 'http://127.0.0.1:11420' },
  webServer: {
    command: 'node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 11420 --strictPort',
    url: 'http://127.0.0.1:11420/e2e.html',
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
