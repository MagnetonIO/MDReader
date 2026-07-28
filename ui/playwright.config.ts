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
 * TWO MODES, and the split matters (docs/testing/gui-e2e.md):
 *   - **mocked** (`e2e.html`, `src/e2e/harness.ts`) — the Tauri IPC answered
 *     in-page from a fixture. Fast, hermetic, independent of indexing, and the
 *     right tool for layout, focus mode, selection and the honest-rendering
 *     rules. It cannot catch a wrong ANSWER: the fixture is the answer.
 *   - **real-engine** (`e2e.html?realengine`, `src/e2e/real-engine.ts`) — the
 *     same shell answered by a GENUINELY RUNNING `catdb-server` over a real
 *     SQLite workspace indexed from `fixtures/corpus`. `globalSetup` below
 *     builds, seeds, indexes, serves and tears that down; a spec never depends
 *     on a server someone started by hand.
 *
 * Neither drives the NATIVE window shell, and neither exercises the Rust Tauri
 * command layer itself: see docs/testing/gui-e2e.md for what that leaves
 * pending.
 */
export default defineConfig({
  testDir: './e2e',
  // Boots the real catdb-server and returns its own teardown. It records a boot
  // failure rather than throwing, so the mocked specs stay independent of the
  // Rust toolchain while the real-engine specs error naming the reason.
  globalSetup: './e2e/support/global-setup.ts',
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
    // The Vite proxy captures MDREADER_E2E_CATDB_PORT when it starts. Reusing a
    // pre-existing dev server could silently route this invocation to another
    // run's CatDB process. Fail closed on an occupied port instead.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
