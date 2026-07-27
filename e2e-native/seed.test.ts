import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, symlink, utimes, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { FLOW, SEMANTIC_KINDS } from './flows.ts';
import {
  clearSeed,
  clearWorkspaceIndex,
  createE2EProfile,
  E2E_BUNDLE_IDENTIFIER,
  fingerprintSourceTree,
  type E2EProfile,
} from './seed.ts';

async function temporaryE2EConfig(): Promise<string> {
  return mkdtemp(join(tmpdir(), 'mdreader-e2e-'));
}

test('clearSeed refuses a forged production profile before it can delete state', async () => {
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

test('clearSeed refuses a forged broad home profile before it can delete state', async () => {
  await assert.rejects(
    () => clearSeed({ identifier: E2E_BUNDLE_IDENTIFIER, configDir: homedir() } as never),
    /validated E2E profile/,
  );
});

test('clearSeed rejects a temp-named symlink before it deletes the target store', async () => {
  const root = await temporaryE2EConfig();
  const target = join(root, 'normal-mdreader-state');
  const configDir = join(root, 'mdreader-e2e-link');
  const sessionPath = join(target, 'session.json');
  await mkdir(target, { recursive: true });
  await writeFile(sessionPath, 'normal-profile-session');
  await symlink(target, configDir);

  try {
    await assert.rejects(
      () => clearSeed({ identifier: E2E_BUNDLE_IDENTIFIER, configDir } as E2EProfile),
      /validated E2E profile/,
    );
    assert.equal(await readFile(sessionPath, 'utf8'), 'normal-profile-session');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('clearSeed rejects a symlinked ancestor before it deletes the target store', async () => {
  const root = await temporaryE2EConfig();
  const target = join(root, 'normal-mdreader-state');
  const redirect = join(root, 'redirect');
  const configDir = join(redirect, 'mdreader-e2e-state');
  const sessionPath = join(target, 'mdreader-e2e-state', 'session.json');
  await mkdir(join(target, 'mdreader-e2e-state'), { recursive: true });
  await writeFile(sessionPath, 'normal-profile-session');
  await symlink(target, redirect);

  try {
    await assert.rejects(
      () => clearSeed({ identifier: E2E_BUNDLE_IDENTIFIER, configDir } as E2EProfile),
      /validated E2E profile/,
    );
    assert.equal(await readFile(sessionPath, 'utf8'), 'normal-profile-session');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('clearWorkspaceIndex rejects a temp-named symlink before recursive deletion', async () => {
  const root = await temporaryE2EConfig();
  const target = join(root, 'normal-mdreader-state');
  const configDir = join(root, 'mdreader-e2e-link');
  const sentinel = join(target, 'workspaces', 'normal-index', 'sentinel.txt');
  await mkdir(join(target, 'workspaces', 'normal-index'), { recursive: true });
  await writeFile(sentinel, 'normal-index');
  await symlink(target, configDir);

  try {
    await assert.rejects(
      () => clearWorkspaceIndex({ identifier: E2E_BUNDLE_IDENTIFIER, configDir } as E2EProfile),
      /validated E2E profile/,
    );
    assert.equal(await readFile(sentinel, 'utf8'), 'normal-index');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('validated test-owned E2E state clears only its persisted state', async () => {
  const configDir = await temporaryE2EConfig();
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, configDir);
  const unrelatedPath = join(configDir, 'keep.txt');
  await mkdir(join(configDir, 'workspaces'), { recursive: true });
  await writeFile(join(configDir, 'session.json'), 'stale session');
  await writeFile(join(configDir, 'settings.json'), 'stale settings');
  await writeFile(unrelatedPath, 'must remain');

  try {
    await clearSeed(profile);
    await clearWorkspaceIndex(profile);
    await assert.rejects(() => readFile(join(configDir, 'session.json')));
    await assert.rejects(() => readFile(join(configDir, 'settings.json')));
    await assert.rejects(() => readFile(join(configDir, 'workspaces')));
    assert.equal(await readFile(unrelatedPath, 'utf8'), 'must remain');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});

test('the shared native flow makes the P8 federated Document surface load-bearing', () => {
  const step = FLOW.find((candidate) => candidate.id === 'federated-document');
  assert.equal(step?.kind, 'inspectFederation');
  assert.equal(SEMANTIC_KINDS.includes('inspectFederation'), true);
});

test('source fingerprints detect an mtime-only touch as well as bytes and membership', async () => {
  const root = await temporaryE2EConfig();
  const document = join(root, 'note.md');
  await writeFile(document, '# unchanged bytes');
  const before = await fingerprintSourceTree(root);

  await utimes(document, new Date('2020-01-01T00:00:00Z'), new Date('2020-01-01T00:00:00Z'));
  const touched = await fingerprintSourceTree(root);
  assert.notDeepEqual(touched, before);
  assert.equal(touched['note.md']?.sha256, before['note.md']?.sha256);

  await writeFile(join(root, 'added.md'), '# added');
  const added = await fingerprintSourceTree(root);
  assert.deepEqual(Object.keys(added), ['added.md', 'note.md']);
  await rm(root, { recursive: true, force: true });
});
