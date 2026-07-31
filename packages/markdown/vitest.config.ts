import { defaultClientConditions } from 'vite';
import { configDefaults, defineConfig } from 'vitest/config';
import base from '@mdreader/config/vitest';

import { workerBundleResolveConditions } from './src/worker-resolve-conditions.js';

// EXACT production mirror: the app's worker/client bundle
// (apps/mdreader/ui/vite.config.ts) resolves with `resolve.conditions:
// workerBundleResolveConditions(defaultClientConditions)`. Call the SAME shared
// helper here so the guard and the shipped app cannot skew. Dropping `worker`
// from the helper reintroduces the crash in both places.
const workerConditions = workerBundleResolveConditions(defaultClientConditions);
// The genuine pre-fix DOM path: `browser` variants, exercised with a real
// injected `document` (see src/dom-globals.setup.ts).
const domConditions = ['browser', ...defaultClientConditions.filter((c) => c !== 'browser')];

// Resolution-specific files; excluded from the default `node` project.
const RESOLUTION_ONLY = ['**/worker-dom-safety.test.ts', '**/dom-worker-parity.test.ts'];
const PARITY = 'src/dom-worker-parity.test.ts';

export default defineConfig({
  test: {
    projects: [
      {
        // Default Node resolution (DOM-free variants) — the full suite.
        test: {
          ...base.test,
          name: 'node',
          environment: 'node',
          exclude: [...configDefaults.exclude, ...RESOLUTION_ONLY],
        },
      },
      {
        // Real DOM path: `browser` conditions + injected jsdom globals, so the
        // pipeline's `browser`/DOM dep variants run their genuine DOM code.
        resolve: { conditions: domConditions },
        ssr: { resolve: { conditions: domConditions } },
        test: {
          ...base.test,
          name: 'dom',
          environment: 'node',
          setupFiles: ['./src/dom-globals.setup.ts'],
          include: [PARITY],
        },
      },
      {
        // Real worker path: `worker` ahead of `browser`, NO document.
        resolve: { conditions: workerConditions },
        ssr: { resolve: { conditions: workerConditions } },
        test: {
          ...base.test,
          name: 'worker',
          environment: 'node',
          include: ['src/worker-dom-safety.test.ts', PARITY],
        },
      },
    ],
  },
});
