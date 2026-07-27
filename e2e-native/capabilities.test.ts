import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  androidCapabilitiesFor,
  iosCapabilitiesFor,
} from './capabilities.ts';
import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

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

test('iOS capabilities bind the exact e2e bundle to a built app', async () => {
  const app = join(tmpdir(), `mdreader-built-${process.pid}.app`);
  await mkdir(app, { recursive: true });
  await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
  await writeFile(join(app, 'MDReader E2E'), 'binary');

  const capabilities = iosCapabilitiesFor(app);
  assert.equal(capabilities['appium:app'], app);
  assert.equal(capabilities['appium:bundleId'], E2E_BUNDLE_IDENTIFIER);
});

test('Android capabilities require a real APK path', async () => {
  assert.throws(() => androidCapabilitiesFor(undefined), /MDREADER_ANDROID_APP/);

  const apk = join(tmpdir(), `mdreader-${process.pid}.apk`);
  await writeFile(apk, '');
  assert.throws(() => androidCapabilitiesFor(apk), /empty/);
  await writeFile(apk, 'test fixture');
  const capabilities = androidCapabilitiesFor(apk);
  assert.equal(capabilities['appium:app'], apk);
  assert.equal(capabilities['appium:appPackage'], E2E_BUNDLE_IDENTIFIER);
});
