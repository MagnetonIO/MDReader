import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { clearSeed, e2eProfile, seedWorkspace } from './seed.ts';

const E2E_BUNDLE_IDENTIFIER = 'dev.mdreader.app.e2e';

async function temporaryE2EConfig(): Promise<string> {
  return mkdtemp(join(tmpdir(), 'mdreader-e2e-'));
}

test('clearSeed refuses the production identifier before it can delete its store', async () => {
  const configDir = await temporaryE2EConfig();
  const sessionPath = join(configDir, 'session.json');
  await writeFile(sessionPath, 'normal-profile-session');

  try {
    await assert.rejects(
      () => clearSeed({ identifier: 'dev.mdreader.app', configDir } as never),
      /validated E2E profile/,
    );
    assert.equal(await readFile(sessionPath, 'utf8'), 'normal-profile-session');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});

test('seedWorkspace refuses a broad home directory before it writes a session', async () => {
  await assert.rejects(
    () => seedWorkspace({ identifier: E2E_BUNDLE_IDENTIFIER, configDir: homedir() } as never, '/fixture'),
    /validated E2E profile/,
  );
});

test('clearSeed refuses the home directory before it can delete persisted state', async () => {
  await assert.rejects(
    () => clearSeed({ identifier: E2E_BUNDLE_IDENTIFIER, configDir: homedir() } as never),
    /validated E2E profile/,
  );
});

test('e2eProfile refuses a test-named symlink to the home directory', async () => {
  const parent = await mkdtemp(join(tmpdir(), 'mdreader-e2e-parent-'));
  const linkPath = join(parent, 'mdreader-e2e-home-link');
  await symlink(homedir(), linkPath);

  try {
    assert.throws(() => e2eProfile(linkPath), /validated E2E profile/);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test('e2eProfile refuses an E2E-looking descendant of a home-directory symlink', async () => {
  const ancestor = join(tmpdir(), `mdreader-e2e-ancestor-${process.pid}`);
  const configDir = join(ancestor, 'mdreader-e2e-profile');
  await symlink(homedir(), ancestor);

  try {
    assert.throws(() => e2eProfile(configDir), /validated E2E profile/);
  } finally {
    await rm(ancestor, { recursive: true, force: true });
  }
});

test('clearSeed clears the store files in a test-owned E2E profile only', async () => {
  const configDir = await temporaryE2EConfig();
  const profile = { identifier: E2E_BUNDLE_IDENTIFIER, configDir } as never;
  const unrelatedPath = join(configDir, 'keep.txt');
  await mkdir(join(configDir, 'workspaces'), { recursive: true });
  await writeFile(join(configDir, 'session.json'), 'stale session');
  await writeFile(join(configDir, 'settings.json'), 'stale settings');
  await writeFile(unrelatedPath, 'must remain');

  try {
    await clearSeed(profile);
    await assert.rejects(() => readFile(join(configDir, 'session.json')));
    await assert.rejects(() => readFile(join(configDir, 'settings.json')));
    await assert.rejects(() => stat(join(configDir, 'workspaces')));
    assert.equal(await readFile(unrelatedPath, 'utf8'), 'must remain');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});
