import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

import { validateReleaseWorkflow } from '../workflow-contract';

const workflowPath = resolve(
  process.cwd(),
  '..',
  '..',
  '.github',
  'workflows',
  'release.yml',
);

function workflow(): Record<string, unknown> {
  return parse(readFileSync(workflowPath, 'utf8')) as Record<string, unknown>;
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

function jobs(
  document: Record<string, unknown>,
): Record<string, Record<string, unknown>> {
  return document.jobs as Record<string, Record<string, unknown>>;
}

function steps(job: Record<string, unknown>): Array<Record<string, unknown>> {
  return job.steps as Array<Record<string, unknown>>;
}

function containsSecret(value: unknown): boolean {
  return JSON.stringify(value).toLowerCase().includes('secrets');
}

describe('fresh credentialed desktop bundle boundary', () => {
  test('keeps application compilation unprotected and moves secrets to a fresh no-checkout bundle job', () => {
    const document = workflow();
    const releaseJobs = jobs(document);
    const build = releaseJobs['build-unsigned'];
    const bundle = releaseJobs['bundle-sign'];
    const verify = releaseJobs.verify;

    expect(build).toBeDefined();
    expect(build?.environment).toBeUndefined();
    expect(containsSecret(build)).toBe(false);
    expect(bundle).toBeDefined();
    expect(bundle?.environment).toBe('${{ matrix.environment }}');
    expect(verify).toBeDefined();

    const bundleSteps = steps(bundle!);
    expect(
      bundleSteps.some((step) =>
        String(step.uses ?? '').startsWith('actions/checkout@'),
      ),
    ).toBe(false);
    expect(
      bundleSteps.some((step) =>
        /\b(?:pnpm|corepack|cargo)\b|tauri\s+build|prepare-direct-release|bundle-direct-release/.test(
          String(step.run ?? ''),
        ),
      ),
    ).toBe(false);

    const verifyBoundary = bundleSteps.findIndex(
      (step) => step.id === 'verify-credential-boundaries',
    );
    const firstSecret = bundleSteps.findIndex(containsSecret);
    expect(verifyBoundary).toBeGreaterThanOrEqual(0);
    expect(firstSecret).toBeGreaterThan(verifyBoundary);
    expect(releaseJobs.aggregate?.needs).toEqual(['prepare', 'verify']);
  });

  test.each([
    ['checkout source', { id: 'checkout', uses: 'actions/checkout@deadbeef' }],
    [
      'execute a build-side wrapper',
      {
        id: 'wrapper',
        run: 'scripts/release/bundle-direct-release.sh target bundles config',
      },
    ],
    [
      'execute checkout node_modules',
      {
        id: 'checkout-cli',
        run: 'node node_modules/@tauri-apps/cli/tauri.js bundle',
      },
    ],
  ])('rejects a credentialed job mutation that can %s', (_label, mutation) => {
    const document = clone(workflow());
    const bundle = jobs(document)['bundle-sign'];
    expect(bundle).toBeDefined();
    const bundleSteps = steps(bundle!);
    const boundary = bundleSteps.findIndex(
      (step) => step.id === 'verify-credential-boundaries',
    );
    bundleSteps.splice(boundary + 1, 0, mutation);

    expect(() => validateReleaseWorkflow(document)).toThrow(
      /credential|checkout|build code|isolated runtime|ordered step set/i,
    );
  });

  test('rejects resolving any private credential before the closed handoff and runtime gate', () => {
    const document = clone(workflow());
    const bundle = jobs(document)['bundle-sign'];
    expect(bundle).toBeDefined();
    const bundleSteps = steps(bundle!);
    const boundary = bundleSteps.findIndex(
      (step) => step.id === 'verify-credential-boundaries',
    );
    bundleSteps.splice(boundary, 0, {
      id: 'early-secret-check',
      env: {
        TAURI_SIGNING_PRIVATE_KEY:
          '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
      },
      run: '[[ -n "$TAURI_SIGNING_PRIVATE_KEY" ]]',
    });

    expect(() => validateReleaseWorkflow(document)).toThrow(
      /credential.*before|secret.*boundary|ordered step set/i,
    );
  });
});
