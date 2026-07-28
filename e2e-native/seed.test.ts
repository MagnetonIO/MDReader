import assert from 'node:assert/strict';
import {
  mkdtemp,
  mkdir,
  readFile,
  realpath,
  rm,
  symlink,
  utimes,
  writeFile,
} from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import test from 'node:test';

import { FLOW, SEMANTIC_KINDS } from './flows.ts';
import {
  clearSeed,
  clearWorkspaceIndex,
  createE2EProfile,
  E2E_BUNDLE_IDENTIFIER,
  E2E_LOCAL_SOURCE_ID,
  fingerprintSourceTree,
  seedWorkspace,
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
  await mkdir(join(configDir, 'managed-delete-staging'), { recursive: true });
  await writeFile(join(configDir, 'session.json'), 'stale session');
  await writeFile(join(configDir, 'settings.json'), 'stale settings');
  await writeFile(join(configDir, 'library.json'), 'stale membership');
  await writeFile(join(configDir, 'library.lock'), 'stale lease');
  await writeFile(join(configDir, 'cloud-library.catdb'), 'stale catalog');
  await writeFile(join(configDir, 'cloud-library.catdb-wal'), 'stale catalog wal');
  await writeFile(join(configDir, 'cloud-library.catdb-shm'), 'stale catalog shm');
  await writeFile(unrelatedPath, 'must remain');

  try {
    await clearSeed(profile);
    await clearWorkspaceIndex(profile);
    await assert.rejects(() => readFile(join(configDir, 'session.json')));
    await assert.rejects(() => readFile(join(configDir, 'settings.json')));
    await assert.rejects(() => readFile(join(configDir, 'workspaces')));
    await assert.rejects(() => readFile(join(configDir, 'managed-delete-staging')));
    await assert.rejects(() => readFile(join(configDir, 'library.json')));
    await assert.rejects(() => readFile(join(configDir, 'library.lock')));
    await assert.rejects(() => readFile(join(configDir, 'cloud-library.catdb')));
    await assert.rejects(() => readFile(join(configDir, 'cloud-library.catdb-wal')));
    await assert.rejects(() => readFile(join(configDir, 'cloud-library.catdb-shm')));
    assert.equal(await readFile(unrelatedPath, 'utf8'), 'must remain');
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});

test('CatDB cleanup targets the isolated data profile when config and data directories differ', async () => {
  const configDir = await temporaryE2EConfig();
  const dataDir = await temporaryE2EConfig();
  const profile = {
    ...createE2EProfile(E2E_BUNDLE_IDENTIFIER, configDir),
    dataDir,
  };
  await writeFile(join(configDir, 'library.json'), 'config-store must remain');
  await writeFile(join(dataDir, 'library.json'), 'stale data registry');
  await mkdir(join(dataDir, 'workspaces'));

  try {
    await clearWorkspaceIndex(profile);
    assert.equal(
      await readFile(join(configDir, 'library.json'), 'utf8'),
      'config-store must remain',
    );
    await assert.rejects(() => readFile(join(dataDir, 'library.json')));
    await assert.rejects(() => readFile(join(dataDir, 'workspaces')));
  } finally {
    await rm(configDir, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  }
});

test('seedWorkspace persists the canonical runner-owned corpus path', async () => {
  const configDir = await temporaryE2EConfig();
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, configDir);
  const corpus = join(configDir, 'fixture-corpus');
  await mkdir(corpus);
  const nonCanonicalCorpus = relative(process.cwd(), corpus);

  try {
    const seedPath = await seedWorkspace(profile, nonCanonicalCorpus);
    const seed = JSON.parse(await readFile(seedPath, 'utf8')) as {
      session?: { root?: unknown };
    };
    assert.equal(seed.session?.root, await realpath(corpus));
  } finally {
    await rm(configDir, { recursive: true, force: true });
  }
});

test('desktop launchers pass the canonical corpus to the isolated backend seed', async () => {
  const macos = await readFile(join(process.cwd(), 'runners', 'macos.ts'), 'utf8');
  const portable = await readFile(
    join(process.cwd(), 'config', 'wdio.desktop.conf.ts'),
    'utf8',
  );

  assert.match(macos, /MDREADER_E2E_CORPUS_ROOT:\s*corpusRoot/);
  assert.match(
    macos,
    /MDREADER_E2E_SOURCE_ID:\s*E2E_LOCAL_SOURCE_ID/g,
  );
  assert.match(
    macos,
    /env:\s*\{[\s\S]*MDREADER_E2E_SOURCE_ID:\s*E2E_LOCAL_SOURCE_ID,[\s\S]*stdio:/,
  );
  assert.match(portable, /process\.env\.MDREADER_E2E_CORPUS_ROOT\s*=\s*corpusRoot/);
  assert.match(
    portable,
    /process\.env\.MDREADER_E2E_SOURCE_ID\s*=\s*E2E_LOCAL_SOURCE_ID/,
  );
  assert.match(E2E_LOCAL_SOURCE_ID, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  assert.ok(
    macos.indexOf('clearWorkspaceIndex(opts.profile)') <
      macos.indexOf('seedWorkspace(opts.profile, corpusRoot)'),
  );
  assert.ok(
    portable.indexOf('clearWorkspaceIndex(E2E_PROFILE)') <
      portable.indexOf('seedWorkspace(E2E_PROFILE, corpusRoot)'),
  );
});

test('a failed native flow is not replaced by a later fixture-count error', async () => {
  const macos = await readFile(join(process.cwd(), 'runners', 'macos.ts'), 'utf8');
  const flow = macos.indexOf('outcome = await runFlow(adapter, consoleLog)');
  const gate = macos.indexOf('if (outcome.ok)', flow);
  const fixtureCheck = macos.indexOf(
    'Connected CatDB fixture did not observe at least two authenticated entity-query POSTs',
    flow,
  );
  assert.ok(flow >= 0 && gate > flow && fixtureCheck > gate);
});

test('the shared native flow makes the P8 federated Document surface load-bearing', () => {
  const step = FLOW.find((candidate) => candidate.id === 'federated-document');
  assert.equal(step?.kind, 'inspectFederation');
  assert.equal(SEMANTIC_KINDS.includes('inspectFederation'), true);
  const libraryStep = FLOW.find((candidate) => candidate.id === 'unified-library-document');
  assert.equal(libraryStep?.kind, 'inspectLibraryDocument');
  assert.equal(SEMANTIC_KINDS.includes('inspectLibraryDocument'), true);
  if (libraryStep?.kind !== 'inspectLibraryDocument') {
    assert.fail('missing unified Library native step');
  }
  assert.match(libraryStep.resultPattern, /Complete/);
  assert.match(libraryStep.sourcesPattern, /ran/);
  assert.match(libraryStep.provenancePattern, /provenance rule/);
  assert.match(libraryStep.schemaPattern, /Provenance rule pin/);
  const partialStep = FLOW.find(
    (candidate) => candidate.id === 'partial-connected-library-document',
  );
  assert.equal(partialStep?.kind, 'inspectPartialLibraryDocument');
  assert.equal(SEMANTIC_KINDS.includes('inspectPartialLibraryDocument'), true);
  if (partialStep?.kind !== 'inspectPartialLibraryDocument') {
    assert.fail('missing partial Connected CatDB native step');
  }
  assert.match(partialStep.membershipsPattern, /Google Drive fixture/);
  assert.equal(partialStep.driveDocument, 'Drive fixture note.md');
  assert.match(partialStep.driveResultPattern, /Complete/);
  assert.match(partialStep.driveSourcesPattern, /Google Drive fixture/);
  assert.match(partialStep.driveProvenancePattern, /\.name/);
  assert.match(partialStep.resultPattern, /Excluded source/);
  assert.match(partialStep.sourcesPattern, /Retry after/);
  assert.match(partialStep.provenancePattern, /Isolated Enterprise fixture/);
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

test('source fingerprints include empty-directory membership', async () => {
  const root = await temporaryE2EConfig();
  const empty = join(root, 'kept-empty');
  await mkdir(empty);
  try {
    const withEmpty = await fingerprintSourceTree(root);
    assert.equal(withEmpty['kept-empty/']?.kind, 'directory');

    await rm(empty, { recursive: true });
    const removed = await fingerprintSourceTree(root);
    assert.notDeepEqual(removed, withEmpty);
    assert.equal(removed['kept-empty/'], undefined);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('a failed native setup still finalizes a fresh run-scoped integrity result', async () => {
  const root = await temporaryE2EConfig();
  const artifacts = await temporaryE2EConfig();
  await writeFile(join(root, 'note.md'), '# unchanged');
  await writeFile(join(artifacts, 'source-fingerprint-after.json'), 'stale-after');
  await writeFile(join(artifacts, 'result.json'), 'stale-result');

  try {
    const seedModule = await import('./seed.ts');
    const runWithArtifacts = (
      seedModule as unknown as {
        withNativeRunArtifacts: (
          base: string,
          source: string,
          run: (runArtifacts: string) => Promise<unknown>,
        ) => Promise<{ artifacts: string; ok: boolean; failedAt: string | null }>;
      }
    ).withNativeRunArtifacts;
    assert.equal(typeof runWithArtifacts, 'function');

    const result = await runWithArtifacts(artifacts, root, async () => {
      throw new Error('session creation failed');
    });
    assert.equal(result.ok, false);
    assert.equal(result.failedAt, 'harness-error');
    assert.notEqual(result.artifacts, artifacts);
    assert.equal(
      typeof JSON.parse(
        await readFile(join(result.artifacts, 'source-fingerprint-before.json'), 'utf8'),
      ),
      'object',
    );
    assert.equal(
      typeof JSON.parse(
        await readFile(join(result.artifacts, 'source-fingerprint-after.json'), 'utf8'),
      ),
      'object',
    );
    const recorded = JSON.parse(await readFile(join(result.artifacts, 'result.json'), 'utf8')) as {
      ok: boolean;
      error: string;
    };
    assert.equal(recorded.ok, false);
    assert.match(recorded.error, /session creation failed/);
    assert.equal(await readFile(join(artifacts, 'result.json'), 'utf8'), 'stale-result');
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(artifacts, { recursive: true, force: true });
  }
});
