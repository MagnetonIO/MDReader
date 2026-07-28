import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  resolveSourceCommit,
  runPnpm,
  stageMobileSourceCommit,
} from './build-e2e.mjs';

export function mobileBuildArgs(platform) {
  const shared = [
    '--filter',
    '@mdreader/app',
    'tauri',
    platform,
    'build',
    '--debug',
  ];
  if (platform === 'ios') {
    return [
      ...shared,
      '--target',
      'aarch64-sim',
      '--features',
      'e2e-build',
      '--config',
      'src-tauri/tauri.e2e.conf.json',
      '--no-sign',
      '--archive-only',
      '--ci',
    ];
  }
  if (platform === 'android') {
    return [
      ...shared,
      '--target',
      'x86_64',
      '--features',
      'e2e-build',
      '--config',
      'src-tauri/tauri.e2e.conf.json',
      '--apk',
      '--ci',
    ];
  }
  throw new Error(`unsupported mobile E2E platform: ${platform}`);
}

export function runMobileBuild(
  platform,
  sourceCommit = resolveSourceCommit(),
  operations = {},
) {
  const stage = operations.stage ?? stageMobileSourceCommit;
  const run = operations.run ?? runPnpm;
  const reportCleanupError =
    operations.reportCleanupError ??
    ((error) => {
      console.error(
        `mobile source-identity cleanup failed: ${
          error instanceof Error ? error.message : error
        }`,
      );
    });
  const cleanup = stage(sourceCommit);
  let childStatus = 0;
  let primaryError = null;
  try {
    childStatus = run(
      ['--filter', '@mdreader/app...', 'run', 'build'],
      sourceCommit,
    );
    if (childStatus === 0) {
      childStatus = run(mobileBuildArgs(platform), sourceCommit);
    }
  } catch (error) {
    primaryError = error;
    throw error;
  } finally {
    try {
      cleanup();
    } catch (error) {
      if (primaryError !== null || childStatus !== 0) {
        reportCleanupError(error);
      } else {
        throw error;
      }
    }
  }
  return childStatus;
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    process.exitCode = runMobileBuild(process.argv[2]);
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
