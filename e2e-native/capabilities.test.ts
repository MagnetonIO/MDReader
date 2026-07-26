import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import test from 'node:test';

const execFileAsync = promisify(execFile);
const E2E_ROOT = dirname(fileURLToPath(import.meta.url));

for (const envName of ['MDREADER_IOS_BUNDLE_ID', 'MDREADER_ANDROID_PACKAGE'] as const) {
  test(`capabilities reject a production ${envName} override`, async () => {
    await assert.rejects(
      () => execFileAsync(process.execPath, ['--experimental-strip-types', '--input-type=module', '-e', "await import('./capabilities.ts')"], {
        cwd: E2E_ROOT,
        env: { ...process.env, [envName]: 'dev.mdreader.app' },
      }),
      /E2E mobile identifier/,
    );
  });
}
