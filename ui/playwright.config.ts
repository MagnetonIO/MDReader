import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright e2e for the MDReader shell. The tests run the real shell + real
 * markdown pipeline in a real browser engine against the Vite dev server. Kept
 * out of the vitest gate — run with `pnpm test:e2e`.
 *
 * Two engines, deliberately:
 *   - **chromium**, which is what the Windows shell (WebView2) is built on;
 *   - **webkit**, which is what the macOS shell (WKWebView) and the Linux shell
 *     (WebKitGTK) are built on — and which has no WebDriver bridge on macOS, so
 *     this is the only place the showcase UI is exercised under WebKit at all.
 *
 * Tauri IPC is answered in-page by the synthetic reader fixture. This checks
 * browser rendering and interaction, not native storage or service retrieval.
 * Native SQLite/service acceptance runs against the reader-owned binaries.
 *
 * Neither drives the NATIVE window shell, and neither exercises the Rust Tauri
 * command layer itself: see docs/testing/gui-e2e.md for what that leaves
 * pending.
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
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
  webServer: {
    command: 'pnpm dev',
    url: 'http://localhost:1420/e2e.html',
    // Keep each acceptance run on its own fixture server.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
