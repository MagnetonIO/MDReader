import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { nativeCliArgs, nativeSyncPreset } from './native-sync-preset.mjs';
import { nativeLaunchEnvironment } from './native-environment.mjs';

test('explicit desktop sync preset stays complete, public, overridable and outside Tauri arguments', () => {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  assert.deepEqual(nativeSyncPreset(root, ['dev']), {});
  const args = ['build', '--debug', '--sync-development', '--no-bundle'];
  assert.deepEqual(nativeCliArgs(args), ['build', '--debug', '--no-bundle']);
  const preset = nativeSyncPreset(root, args);
  assert.equal(Object.keys(preset).length, 9);
  assert.equal(preset.MDREADER_NATIVE_SYNC_STAGE, 'development');
  assert.equal(preset.MDREADER_NATIVE_SYNC_REDIRECT_URI, 'http://127.0.0.1:43891/mdreader-sync/callback');
  assert.equal(new URL(preset.MDREADER_NATIVE_SYNC_AUTHORIZATION_ENDPOINT).origin,
    new URL(preset.MDREADER_NATIVE_SYNC_ISSUER).origin);
  assert.equal(nativeLaunchEnvironment(root, args, { MDREADER_NATIVE_SYNC_STAGE: 'disabled' })
    .MDREADER_NATIVE_SYNC_STAGE, 'disabled');
  assert.equal(nativeLaunchEnvironment(root, args, { MDREADER_NATIVE_SYNC_CLIENT_ID: '' })
    .MDREADER_NATIVE_SYNC_CLIENT_ID, '');
  for (const invalid of [['dev', '--sync-development', '--sync-development'], ['ios', 'build', '--sync-development']]) {
    assert.throws(() => nativeCliArgs(invalid));
  }
});
