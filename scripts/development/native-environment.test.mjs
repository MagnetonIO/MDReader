import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { nativeLaunchEnvironment } from './native-environment.mjs';

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
