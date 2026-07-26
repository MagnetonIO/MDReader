import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { clearSeed, createE2EProfile, E2E_BUNDLE_IDENTIFIER } from './seed.ts';

async function temporaryE2EConfig(): Promise<string> {
  return mkdtemp(join(tmpdir(), 'mdreader-e2e-'));
}

test('production identifiers are refused before native seed cleanup can delete their state', async () => {
  const configDir = await temporaryE2EConfig();
  const sessionPath = join(configDir, 'session.json');
  await writeFile(sessionPath, 'normal-profile-session');

  try {
    assert.throws(
      () => createE2EProfile('dev.mdreader.app', configDir),
      /validated E2E profile/,
    );
    assert.equal(await readFile(sessionPath, 'utf8'), 'normal-profile-session');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});

test('broad home paths are refused before native seed cleanup can delete state', () => {
  assert.throws(
    () => createE2EProfile(E2E_BUNDLE_IDENTIFIER, homedir()),
    /validated E2E profile/,
  );
});

test('validated test-owned E2E state clears only its persisted store files', async () => {
  const configDir = await temporaryE2EConfig();
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, configDir);
  const unrelatedPath = join(configDir, 'keep.txt');
  await mkdir(join(configDir, 'workspaces'), { recursive: true });
  await writeFile(join(configDir, 'session.json'), 'stale session');
  await writeFile(join(configDir, 'settings.json'), 'stale settings');
  await writeFile(unrelatedPath, 'must remain');

  try {
    await clearSeed(profile);
    await assert.rejects(() => readFile(join(configDir, 'session.json')));
    await assert.rejects(() => readFile(join(configDir, 'settings.json')));
    assert.equal(await readFile(unrelatedPath, 'utf8'), 'must remain');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});
