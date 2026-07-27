import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';

import {
  androidCapabilitiesFor,
  iosCapabilitiesFor,
} from './capabilities.ts';
import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

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

    const capabilities = iosCapabilitiesFor(app);
    assert.equal(capabilities['appium:app'], app);
    assert.equal(capabilities['appium:bundleId'], E2E_BUNDLE_IDENTIFIER);
    await rm(root, { recursive: true });
  },
);

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
