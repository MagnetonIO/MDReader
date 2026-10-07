import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defaultClientConditions, loadConfigFromFile } from 'vite';
import { beforeAll, describe, expect, test } from 'vitest';

import {
  WORKER_BUNDLE_RESOLVE_CONDITIONS,
  workerBundleResolveConditions,
} from './worker-resolve-conditions.js';

/**
 * Resolver-mirror guard. ONE source of truth —
 * {@link workerBundleResolveConditions} — is consumed by the app's Vite bundle
 * (`apps/mdreader/ui/vite.config.ts`) and the `worker` vitest project
 * (`vitest.config.ts`). This test:
 *
 *  1. pins the helper's output to the production condition list, so a Vite bump
 *     that changes `defaultClientConditions`, or an edit to
 *     WORKER_BUNDLE_RESOLVE_CONDITIONS, fails loudly here; and
 *  2. guards the ACTUAL production usage SEMANTICALLY — it loads and EVALUATES
 *     the shipped `vite.config.ts` with Vite's own config loader and asserts the
 *     effective `resolve.conditions` array deep-equals the helper output. Because
 *     it evaluates the real config object (not its source text), a hand-edit that
 *     hardcodes `resolve.conditions` — even one that leaves the old helper
 *     import/call commented out above it — no longer matches and turns this red.
 *     (Requires the workspace to be built first, which the gate does:
 *     `vite.config.ts` imports the helper from `@mdreader/markdown/worker-conditions`.)
 */

const viteConfigPath = fileURLToPath(new URL('../../../ui/vite.config.ts', import.meta.url));

describe('worker resolve conditions mirror production', () => {
  const effective = workerBundleResolveConditions(defaultClientConditions);

  let productionConditions: readonly string[] | undefined;

  beforeAll(async () => {
    const loaded = await loadConfigFromFile(
      { command: 'build', mode: 'production' },
      viteConfigPath,
      dirname(viteConfigPath),
      'silent',
    );
    if (loaded == null) throw new Error(`could not load ${viteConfigPath}`);
    productionConditions = loaded.config.resolve?.conditions;
  });

  test('the DOM-free `worker` condition is prepended', () => {
    expect(WORKER_BUNDLE_RESOLVE_CONDITIONS).toEqual(['worker']);
    expect(effective[0]).toBe('worker');
  });

  test('effective condition list equals production (worker + Vite client defaults)', () => {
    // Vite 7's client defaults; the app pins vite ^7 (same major as this package)
    // so the two lists cannot diverge by Vite version.
    expect(defaultClientConditions).toEqual(['module', 'browser', 'development|production']);
    expect(effective).toEqual(['worker', 'module', 'browser', 'development|production']);
  });

  test("the shipped vite.config.ts's EVALUATED resolve.conditions equals the shared helper output", () => {
    // Semantic, not a source grep: this is the real production config object,
    // loaded and evaluated by Vite. Drift in what the app actually ships fails here.
    expect(productionConditions).toEqual(effective);
  });
});
