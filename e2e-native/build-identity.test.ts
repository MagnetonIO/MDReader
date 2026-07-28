import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');
const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const MOBILE_SOURCE_COMMIT_FILE = resolve(
  REPO_ROOT,
  'apps',
  'mdreader',
  'src-tauri',
  '.e2e-source-commit',
);
const CARGO_ARGS = [
  'check',
  '-p',
  'mdreader-app',
  '--no-default-features',
  '--features',
  'e2e-build',
  '--lib',
];

function cargoCheck(
  tauriConfig: string | undefined,
  sourceCommit: string | undefined = undefined,
) {
  const env = { ...process.env };
  if (tauriConfig === undefined) delete env.TAURI_CONFIG;
  else env.TAURI_CONFIG = tauriConfig;
  if (sourceCommit === undefined) delete env.MDREADER_SOURCE_COMMIT;
  else env.MDREADER_SOURCE_COMMIT = sourceCommit;
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

    const isolatedConfig = JSON.stringify({
      productName: 'MDReader E2E',
      mainBinaryName: 'mdreader-e2e',
      identifier: 'dev.mdreader.app.e2e',
    });
    const unbound = cargoCheck(isolatedConfig);
    assert.notEqual(unbound.status, 0, 'unbound E2E source unexpectedly compiled');
    assert.match(
      `${unbound.stdout}\n${unbound.stderr}`,
      /e2e-build requires MDREADER_SOURCE_COMMIT/i,
    );

    const isolated = cargoCheck(
      isolatedConfig,
      SOURCE_COMMIT,
    );
    assert.equal(
      isolated.status,
      0,
      `isolated E2E config did not compile:\n${isolated.stderr}`,
    );

    writeFileSync(MOBILE_SOURCE_COMMIT_FILE, `${SOURCE_COMMIT}\n`, {
      flag: 'wx',
      mode: 0o600,
    });
    try {
      const mobileHandoff = cargoCheck(isolatedConfig);
      assert.equal(
        mobileHandoff.status,
        0,
        `mobile source-identity handoff did not compile:\n${mobileHandoff.stderr}`,
      );

      const conflict = cargoCheck(
        isolatedConfig,
        '89abcdef0123456789abcdef0123456789abcdef',
      );
      assert.notEqual(
        conflict.status,
        0,
        'conflicting E2E source identities unexpectedly compiled',
      );
      assert.match(
        `${conflict.stdout}\n${conflict.stderr}`,
        /E2E source commit identities do not match/i,
      );
    } finally {
      rmSync(MOBILE_SOURCE_COMMIT_FILE, { force: true });
    }
  },
);
