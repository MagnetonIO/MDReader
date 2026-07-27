import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');
const CARGO_ARGS = [
  'check',
  '-p',
  'mdreader-app',
  '--no-default-features',
  '--features',
  'e2e-build',
  '--lib',
];

function cargoCheck(tauriConfig: string | undefined) {
  const env = { ...process.env };
  if (tauriConfig === undefined) delete env.TAURI_CONFIG;
  else env.TAURI_CONFIG = tauriConfig;
  return spawnSync('cargo', CARGO_ARGS, {
    cwd: REPO_ROOT,
    env,
    encoding: 'utf8',
    maxBuffer: 5 * 1024 * 1024,
  });
}

test(
  'e2e-build requires the isolated dev.mdreader.app.e2e Tauri identity',
  { timeout: 300_000 },
  () => {
    const production = cargoCheck(undefined);
    assert.notEqual(
      production.status,
      0,
      'production Tauri config unexpectedly compiled with e2e-build',
    );
    assert.match(
      `${production.stdout}\n${production.stderr}`,
      /e2e-build requires Tauri bundle identity dev\.mdreader\.app\.e2e/i,
    );

    const isolated = cargoCheck(
      JSON.stringify({
        productName: 'MDReader E2E',
        mainBinaryName: 'mdreader-e2e',
        identifier: 'dev.mdreader.app.e2e',
      }),
    );
    assert.equal(
      isolated.status,
      0,
      `isolated E2E config did not compile:\n${isolated.stderr}`,
    );
  },
);
