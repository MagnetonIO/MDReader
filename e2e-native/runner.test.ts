import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { chmod, mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import test from 'node:test';

const execFileAsync = promisify(execFile);
const E2E_ROOT = dirname(fileURLToPath(import.meta.url));

test('macOS runner rejects a production MDReader bundle before it can seed or launch it', async () => {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-e2e-runner-'));
  const app = join(root, 'MDReader.app');
  const binary = join(app, 'Contents', 'MacOS', 'mdreader-app');
  await mkdir(dirname(binary), { recursive: true });
  await writeFile(binary, '#!/bin/sh\nexit 0\n');
  await chmod(binary, 0o755);
  await writeFile(
    join(app, 'Contents', 'Info.plist'),
    '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.mdreader.app</string></dict></plist>',
  );

  try {
    await assert.rejects(
      () => execFileAsync(process.execPath, ['--experimental-strip-types', 'runners/macos.ts', '--app', app, '--corpus', join(root, 'missing')], { cwd: E2E_ROOT }),
      (error: { stdout?: string; stderr?: string }) => {
        assert.match(`${error.stdout ?? ''}${error.stderr ?? ''}`, /E2E bundle identifier/);
        return true;
      },
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
