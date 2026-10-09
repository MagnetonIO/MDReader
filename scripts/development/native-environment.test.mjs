import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { nativeEnvironment, nativeLaunchEnvironment } from './native-environment.mjs';

test('native desktop token configuration loads without importing tokens or Vite secrets', () => {
  const root = mkdtempSync(join(tmpdir(), 'mdreader-token-config-'));
  try {
    writeFileSync(join(root, '.env.local'), 'MDREADER_GOOGLE_DRIVE_CLIENT_SECRET=synthetic-desktop-parameter\nVITE_GOOGLE_CLIENT_SECRET=synthetic-renderer-injection\nGOOGLE_ACCESS_TOKEN=synthetic-user-token\n');
    const env = nativeEnvironment(root, {});
    assert.equal(env.MDREADER_GOOGLE_DRIVE_CLIENT_SECRET, 'synthetic-desktop-parameter');
    assert.equal(env.VITE_GOOGLE_CLIENT_SECRET, undefined);
    assert.equal(env.GOOGLE_ACCESS_TOKEN, undefined);
    assert.equal(nativeEnvironment(root, { MDREADER_GOOGLE_DRIVE_CLIENT_SECRET: '' }).MDREADER_GOOGLE_DRIVE_CLIENT_SECRET, '');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('native launch pins its checkout and preserves only a selected iOS SDK', () => {
  const root = mkdtempSync(join(tmpdir(), 'mdreader-launch-'));
  try {
    const inherited = {
      TAURI_APP_PATH: '/other-checkout/src-tauri', TAURI_FRONTEND_PATH: '/other-checkout',
      SDKROOT: '/MacOSX.sdk', CARGO_BUILD_JOBS: '2',
    };
    const tools = () => ({ rustc: '/pinned/bin/rustc', rustdoc: '/pinned/bin/rustdoc' });
    const build = nativeLaunchEnvironment(root, ['ios', 'build'], inherited, tools);
    assert.equal(build.TAURI_APP_PATH, join(root, 'src-tauri'));
    assert.equal(build.TAURI_FRONTEND_PATH, root);
    assert.equal(build.SDKROOT, undefined);
    assert.equal(build.CARGO_BUILD_RUSTC, '/pinned/bin/rustc');
    assert.equal(build.CARGO_BUILD_RUSTDOC, '/pinned/bin/rustdoc');
    const nested = nativeLaunchEnvironment(root, ['ios', 'xcode-script'], {
      ...inherited, SDKROOT: '/iPhoneSimulator.sdk',
    }, tools);
    assert.equal(nested.TAURI_APP_PATH, join(root, 'src-tauri'));
    assert.equal(nested.SDKROOT, '/iPhoneSimulator.sdk');
    assert.equal(nativeLaunchEnvironment(root, ['build'], inherited).SDKROOT, '/MacOSX.sdk');
    assert.equal(inherited.TAURI_APP_PATH, '/other-checkout/src-tauri');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
