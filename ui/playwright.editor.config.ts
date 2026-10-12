import { defineConfig } from '@playwright/test';
import base from './playwright.config';
/** Real browser/source interactions; native storage is a separate acceptance. */
export default defineConfig({
  ...base, testMatch: 'editor-batch.spec.ts', fullyParallel: false, workers: 1,
  // Use the actual browser host identity: Desktop Chrome's Windows UA on a Mac
  // otherwise disagrees with CodeMirror's native platform keyboard detection.
  projects: base.projects?.map(project => project.name === 'chromium'
    ? { ...project, use: { ...project.use, userAgent: undefined } } : project),
  use: { ...base.use, baseURL: 'http://127.0.0.1:11420' },
  webServer: { command: 'node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 11420 --strictPort',
    url: 'http://127.0.0.1:11420/e2e.html', reuseExistingServer: false, timeout: 30_000 },
});
