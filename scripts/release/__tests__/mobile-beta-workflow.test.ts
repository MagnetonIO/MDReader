import { execFileSync } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';

import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

import { validateMobileBetaWorkflow } from '../mobile-beta-workflow-contract';

const root = resolve(process.cwd(), '..', '..');
const workflowPath = resolve(
  root,
  '.github',
  'workflows',
  'mobile-beta.yml',
);

function workflow(): Record<string, unknown> {
  return parse(readFileSync(workflowPath, 'utf8')) as Record<string, unknown>;
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

function jobs(document: Record<string, unknown>) {
  return document.jobs as Record<string, Record<string, unknown>>;
}

function steps(job: Record<string, unknown>) {
  return job.steps as Array<Record<string, unknown>>;
}

describe('production mobile beta workflow', () => {
  test('tracked workflow satisfies the fail-closed beta contract', () => {
    expect(() => validateMobileBetaWorkflow(workflow())).not.toThrow();
  });

  test('contract rejects E2E identity, credential leakage into builds, and an unprotected publisher', () => {
    const e2e = clone(workflow());
    const iosBuild = steps(jobs(e2e)['build-ios-unsigned']!).find(
      (step) => step.id === 'build-ios-archive',
    )!;
    iosBuild.run = String(iosBuild.run).replace(
      'dev.mdreader.app',
      'dev.mdreader.app.e2e',
    );
    expect(() => validateMobileBetaWorkflow(e2e)).toThrow();

    const leaked = clone(workflow());
    steps(jobs(leaked)['build-android-unsigned']!)[0]!.env = {
      LEAK: '${{ secrets.MDREADER_ANDROID_UPLOAD_STORE_PASSWORD }}',
    };
    expect(() => validateMobileBetaWorkflow(leaked)).toThrow();

    const unprotected = clone(workflow());
    delete jobs(unprotected)['publish-ios-beta']!.environment;
    expect(() => validateMobileBetaWorkflow(unprotected)).toThrow();
  });

  test('uses pinned actions and executable system-Bash-compatible signing scripts', () => {
    for (const job of Object.values(jobs(workflow()))) {
      for (const step of steps(job)) {
        if (typeof step.uses === 'string') {
          expect(step.uses).toMatch(/@[0-9a-f]{40}$/);
        }
      }
    }

    for (const script of ['sign-ios-beta.sh', 'sign-android-beta.sh']) {
      const path = resolve(root, 'scripts', 'release', script);
      expect(statSync(path).mode & 0o111).not.toBe(0);
      expect(() => execFileSync('/bin/bash', ['-n', path])).not.toThrow();
      expect(readFileSync(path, 'utf8')).not.toContain('mapfile');
      if (script === 'sign-ios-beta.sh') {
        expect(readFileSync(path, 'utf8')).toContain("read -r -d ''");
        expect(readFileSync(path, 'utf8')).toContain('-print0');
      }
    }
  });

  test('resolves and binds the Google Play track only inside the protected publisher', () => {
    const tracked = workflow();
    const prepare = jobs(tracked).prepare!;
    expect(JSON.stringify(prepare)).not.toContain(
      'MDREADER_GOOGLE_PLAY_CLOSED_TRACK',
    );
    expect(JSON.stringify(prepare)).not.toContain('google_closed_track');

    const publisher = jobs(tracked)['publish-android-beta']!;
    const inspection = steps(publisher).find(
      (step) => step.id === 'inspect-android',
    )!;
    expect(inspection.env).toEqual({
      GOOGLE_CLOSED_TRACK: '${{ vars.MDREADER_GOOGLE_PLAY_CLOSED_TRACK }}',
    });
    expect(String(inspection.run)).toContain(
      '--closed-track "$GOOGLE_CLOSED_TRACK"',
    );
    const upload = steps(publisher).find(
      (step) => step.id === 'upload-google-play',
    )!;
    expect(String(upload.run)).not.toContain('--closed-track');
    expect(String(upload.run)).not.toContain('--version-code');
    expect(String(upload.run)).not.toContain('--release-name');

    const rebound = clone(tracked);
    steps(jobs(rebound)['publish-android-beta']!).find(
      (step) => step.id === 'inspect-android',
    )!.env = {
      GOOGLE_CLOSED_TRACK: '${{ needs.prepare.outputs.google_closed_track }}',
    };
    expect(() => validateMobileBetaWorkflow(rebound)).toThrow(
      /protected.*track|track.*protected/i,
    );
  });

  test('builds production device packages without updater, E2E, debug, APK, or direct-database authority', () => {
    const source = readFileSync(workflowPath, 'utf8');
    expect(source).toContain('dev.mdreader.app');
    expect(source).not.toContain('dev.mdreader.app.e2e');
    expect(source).not.toContain('e2e-build');
    expect(source).not.toContain('direct-updater');
    expect(source).not.toMatch(/\s--debug(?:\s|$)/);
    expect(source).not.toMatch(/\s--apk(?:\s|$)/);
    expect(source).not.toMatch(
      /DATABASE_URL|POSTGRES_URL|SNOWFLAKE|WAREHOUSE_PASSWORD/,
    );
    expect(source).toContain('--target aarch64');
    expect(source).toContain('--no-sign');
    expect(source).toContain('--archive-only');
    expect(source).toContain('--aab');
  });

  test('keeps production and E2E mobile identities separate in Tauri configuration', () => {
    const production = JSON.parse(
      readFileSync(
        resolve(root, 'apps', 'mdreader', 'src-tauri', 'tauri.conf.json'),
        'utf8',
      ),
    );
    const e2e = JSON.parse(
      readFileSync(
        resolve(root, 'apps', 'mdreader', 'src-tauri', 'tauri.e2e.conf.json'),
        'utf8',
      ),
    );
    expect(production.identifier).toBe('dev.mdreader.app');
    expect(e2e.identifier).toBe('dev.mdreader.app.e2e');
  });
});
