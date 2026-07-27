import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { appendFile, chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';

import {
  androidCapabilitiesFor,
  iosCapabilitiesFor,
} from './capabilities.ts';
import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';

function buildManifest(sourceCommit = SOURCE_COMMIT): string {
  return `MDREADER_E2E_BUILD_MANIFEST:${JSON.stringify({
      schema: 1,
      source_commit: sourceCommit,
      bundle_identifier: E2E_BUNDLE_IDENTIFIER,
      profile: 'e2e-build',
    })}:MDREADER_E2E_BUILD_MANIFEST_END`;
}

const exec = promisify(execFile);

function plist(bundleIdentifier: string, executable: string): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${bundleIdentifier}</string>
<key>CFBundleExecutable</key><string>${executable}</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>`;
}

test('iOS capabilities reject a missing or incomplete simulator app', async () => {
  assert.throws(() => iosCapabilitiesFor(undefined), /MDREADER_IOS_APP/);

  const incomplete = join(tmpdir(), `mdreader-incomplete-${process.pid}.app`);
  await mkdir(incomplete, { recursive: true });
  assert.throws(() => iosCapabilitiesFor(incomplete), /Info\.plist/);
});

test('iOS capabilities reject malformed and production-identity bundles', async () => {
  const app = join(tmpdir(), `mdreader-production-${process.pid}.app`);
  await mkdir(app, { recursive: true });
  await writeFile(join(app, 'Info.plist'), 'not a plist');
  assert.throws(() => iosCapabilitiesFor(app), /property list/);

  await writeFile(join(app, 'Info.plist'), plist('dev.mdreader.app', 'MDReader'));
  await writeFile(join(app, 'MDReader'), 'binary');
  assert.throws(() => iosCapabilitiesFor(app), /dev\.mdreader\.app\.e2e/);
});

test('iOS capabilities reject a text executable behind a valid-looking plist', async () => {
  const app = join(tmpdir(), `mdreader-spoofed-${process.pid}.app`);
  await mkdir(app, { recursive: true });
  await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
  await writeFile(join(app, 'MDReader E2E'), 'not a Mach-O executable');
  await chmod(join(app, 'MDReader E2E'), 0o755);

  assert.throws(() => iosCapabilitiesFor(app), /Mach-O|Simulator/);
});

test(
  'iOS capabilities bind the exact e2e bundle to a real arm64 Simulator executable',
  { skip: process.platform !== 'darwin' },
  async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-ios-artifact-'));
    const app = join(root, 'MDReader E2E.app');
    const source = join(root, 'main.c');
    const executable = join(app, 'MDReader E2E');
    await mkdir(app, { recursive: true });
    await writeFile(source, 'int main(void) { return 0; }\n');
    await exec('/usr/bin/xcrun', [
      '--sdk',
      'iphonesimulator',
      'clang',
      '-target',
      'arm64-apple-ios14.0-simulator',
      source,
      '-o',
      executable,
    ]);
    await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
    await appendFile(executable, buildManifest());

    const capabilities = iosCapabilitiesFor(app, SOURCE_COMMIT);
    assert.equal(capabilities['appium:app'], app);
    assert.equal(capabilities['appium:bundleId'], E2E_BUNDLE_IDENTIFIER);
    await rm(root, { recursive: true });
  },
);

test('iOS capabilities reject an otherwise valid app built from a stale source commit', async () => {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-ios-stale-'));
  const app = join(root, 'MDReader E2E.app');
  const source = join(root, 'main.c');
  const executable = join(app, 'MDReader E2E');
  await mkdir(app, { recursive: true });
  await writeFile(source, 'int main(void) { return 0; }\n');
  await exec('/usr/bin/xcrun', [
    '--sdk',
    'iphonesimulator',
    'clang',
    '-target',
    'arm64-apple-ios14.0-simulator',
    source,
    '-o',
    executable,
  ]);
  await chmod(executable, 0o755);
  await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
  await appendFile(executable, buildManifest('f'.repeat(40)));

  assert.throws(() => iosCapabilitiesFor(app, SOURCE_COMMIT), /source commit/i);
  await rm(root, { recursive: true });
});

test('Android capabilities require a real APK path', async () => {
  assert.throws(() => androidCapabilitiesFor(undefined), /MDREADER_ANDROID_APP/);

  const apk = join(tmpdir(), `mdreader-${process.pid}.apk`);
  await writeFile(apk, '');
  assert.throws(() => androidCapabilitiesFor(apk), /empty/);
  await writeFile(apk, 'test fixture');
  assert.throws(() => androidCapabilitiesFor(apk), /APK|ZIP/);
});

test('Android capabilities reject a ZIP-shaped package without a compiled manifest', async () => {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-android-spoof-'));
  const payload = join(root, 'payload');
  const apk = join(root, 'spoof.apk');
  await mkdir(payload);
  await writeFile(
    join(payload, 'AndroidManifest.xml'),
    `<manifest package="${E2E_BUNDLE_IDENTIFIER}"></manifest>`,
  );
  await exec('/usr/bin/zip', ['-qr', apk, '.'], { cwd: payload });

  assert.throws(() => androidCapabilitiesFor(apk), /manifest|APK/i);
  await rm(root, { recursive: true });
});
