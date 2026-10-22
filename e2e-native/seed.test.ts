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
import { DatabaseSync } from 'node:sqlite';
import { join, relative } from 'node:path';
import test from 'node:test';

import { WebviewAdapter } from './drivers/webview-adapter.ts';
import { FLOW } from './flows.ts';
import { mobileDeviceFixture } from './mobile-device-fixture.ts';
import { checkPngFixture } from './png-fixture.ts';
import {
  clearSeed,
  clearWorkspaceIndex,
  createE2EProfile,
  E2E_BUNDLE_IDENTIFIER,
  E2E_LOCAL_SOURCE_ID,
  fingerprintSourceTree,
  seedWindowsWorkspace,
  seedWorkspace,
  WINDOWS_STATE_APPLICATION_ID,
  WINDOWS_STATE_DATABASE,
  WINDOWS_STATE_TABLES,
  type E2EProfile,
} from './seed.ts';

async function temporaryE2EConfig(): Promise<string> {
  return mkdtemp(join(tmpdir(), 'mdreader-e2e-'));
}

test('mobile image preflight accepts the public corpus PNG and refuses corrupt image data before device upload', () => {
  const png = mobileDeviceFixture().images.find(image => image.mime === 'image/png')!.bytes;
  assert.equal(png.readUInt32BE(16), 2);
  assert.equal(png.readUInt32BE(20), 3);
  const corrupt = Buffer.from(png);
  corrupt[45] = corrupt[45]! ^ 1;
  assert.throws(() => checkPngFixture(corrupt), /PNG fixture is corrupt/);
  assert.throws(() => checkPngFixture(png.subarray(0, -1)), /PNG fixture is corrupt/);
});

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

test('seedWindowsWorkspace writes the exact native Windows journal schema and session', async () => {
  const schema = await readFile(
    join(process.cwd(), '..', 'src-tauri', 'src', 'profile_transition', 'persistence', 'windows_files', 'schema.rs'),
    'utf8',
  );
  for (const sql of WINDOWS_STATE_TABLES) assert.ok(schema.includes(JSON.stringify(sql)), sql);
  assert.match(schema, new RegExp(`const APPLICATION: u32 = 0x${WINDOWS_STATE_APPLICATION_ID.toString(16)};`));
  assert.match(
    await readFile(join(process.cwd(), '..', 'src-tauri', 'src', 'profile_transition', 'persistence', 'windows_files.rs'), 'utf8'),
    new RegExp(`const DATABASE: &str = "${WINDOWS_STATE_DATABASE.replace('.', '\\.')}";`),
  );

  const configDir = await temporaryE2EConfig();
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, configDir);
  const corpus = join(configDir, 'fixture-corpus');
  await mkdir(corpus);
  try {
    const path = await seedWindowsWorkspace(profile, corpus);
    assert.equal(path, join(await realpath(configDir), WINDOWS_STATE_DATABASE));
    // A fresh owner refuses legacy JSON beside it, and WAL sidecars outright.
    for (const absent of ['session.json', `${WINDOWS_STATE_DATABASE}-wal`, `${WINDOWS_STATE_DATABASE}-shm`]) {
      await assert.rejects(() => readFile(join(configDir, absent)));
    }
    const database = new DatabaseSync(path, { readOnly: true });
    try {
      const pragma = (name: string) =>
        Object.values(database.prepare(`PRAGMA ${name}`).get()!)[0];
      assert.equal(pragma('application_id'), WINDOWS_STATE_APPLICATION_ID);
      assert.equal(pragma('page_size'), 4096);
      const tables = database
        .prepare("SELECT sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .all()
        .map((row) => row.sql);
      assert.deepEqual(tables, [...WINDOWS_STATE_TABLES]);
      assert.deepEqual(
        { ...database.prepare('SELECT singleton,version FROM reader_json_meta').get() },
        { singleton: 1, version: 1 },
      );
      const row = database
        .prepare("SELECT typeof(body) AS kind,body FROM reader_json WHERE name='session.json'")
        .get()!;
      assert.equal(row.kind, 'blob');
      const seed = JSON.parse(Buffer.from(row.body as Uint8Array).toString('utf8')) as {
        session?: { root?: unknown };
      };
      assert.equal(seed.session?.root, await realpath(corpus));
    } finally {
      database.close();
    }
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
  assert.match(portable, /process\.env\.MDREADER_E2E_CORPUS_ROOT\s*=\s*corpusRoot/);
  assert.match(
    portable,
    /process\.env\.MDREADER_E2E_SOURCE_ID\s*=\s*E2E_LOCAL_SOURCE_ID/,
  );
  assert.match(E2E_LOCAL_SOURCE_ID, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  assert.match(macos, /MDREADER_E2E_PROFILE_ROOT:\s*opts\.profile\.dataDir/);
  assert.match(portable, /process\.env\.MDREADER_E2E_PROFILE_ROOT\s*=\s*readerFixture\.profile\.dataDir/);
  assert.equal(macos.includes('seed_reviewed_workspace'), false);

});

// Windows checkouts may convert to CRLF; the source patterns below use LF.
async function runnerSource(): Promise<string> {
  const source = await readFile(join(process.cwd(), 'drivers', 'runner.ts'), 'utf8');
  return source.replace(/\r\n/g, '\n');
}

test('the file palette proves a result is ready before selecting it', async () => {
  const runner = await runnerSource();
  const openDoc = runner.slice(
    runner.indexOf("case 'openDoc':"),
    runner.indexOf("case 'search':"),
  );
  const open = openDoc.indexOf("openSurface(\n        d,\n        'Mod+P'");
  const focus = openDoc.indexOf('const inputEntered = await enterInput(');
  const result = openDoc.indexOf("await d.countClass('mdsearch__item')");
  const select = openDoc.indexOf("await d.chord('Enter')");
  assert.ok(open >= 0 && focus > open && result > focus && select > result);
});

test('the search palette proves it opened before typing the query', async () => {
  const runner = await runnerSource();
  const search = runner.slice(
    runner.indexOf("case 'search':"),
    runner.indexOf("case 'jumpToHit':"),
  );
  const open = search.indexOf("openSurface(\n        d,\n        'Mod+K'");
  const palette = search.indexOf("'mdsearch'", open);
  const type = search.indexOf('const inputEntered = await enterInput(');
  const result = search.indexOf('await d.countClass(rowClass)');
  assert.ok(open >= 0 && palette > open && type > palette && result > type);
});

test('direct native text delivery is only a measured fallback after an empty primary read', async () => {
  const runner = await runnerSource();
  const enter = runner.slice(
    runner.indexOf('async function enterInput('),
    runner.indexOf('/**\n * Run the whole shared FLOW', runner.indexOf('async function enterInput(')),
  );
  const primary = enter.indexOf('await d.type(value)');
  const read = enter.indexOf('await waitUntil(readsExpectedValue, 1_500)');
  const empty = enter.indexOf("actual.trim() === ''");
  const direct = enter.indexOf('await d.typeDirect(value)');
  const verify = enter.indexOf('await waitUntil(readsExpectedValue, 5_000)', direct);
  assert.ok(primary >= 0 && read > primary && empty > read && direct > empty && verify > direct);
});

test('webview typing presses and releases every character, including repeated letters', async () => {
  const sent: string[] = [];
  const keys = {
    down(value: string) { sent.push(`down ${value}`); return keys; },
    up(value: string) { sent.push(`up ${value}`); return keys; },
    async perform() { sent.push('perform'); },
  };
  const browser = { action: (kind: string) => { assert.equal(kind, 'key'); return keys; } };
  const adapter = new WebviewAdapter({
    browser: browser as unknown as WebdriverIO.Browser,
    name: 'fake',
    windowPendingReason: 'fake',
  });
  await adapter.type('pluggable');
  assert.deepEqual(sent, [...'pluggable'].flatMap((char) => [`down ${char}`, `up ${char}`]).concat('perform'));
});

test('the Windows fixture applies the exact private descriptor the native owner creates', async () => {
  const fixture = await readFile(join(process.cwd(), 'desktop-fixture.ts'), 'utf8');
  const native = await readFile(
    join(process.cwd(), '..', 'crates', 'mdreader-capture', 'src', 'platform', 'windows', 'security.rs'),
    'utf8',
  );
  const nativeSddl = /format!\("(O:\{sid\}D:P[^"]+)"\)/.exec(native)?.[1];
  const fixtureSddl = /SetSecurityDescriptorSddlForm\(\\"(O:\$\{sid\}D:P[^\\]+)\\"/.exec(fixture)?.[1];
  assert.ok(nativeSddl && fixtureSddl);
  assert.equal(
    fixtureSddl.replace(/\$\{sid\}|\$sid/g, 'SID').replace(/\$flags/g, 'OICI'),
    nativeSddl.replace(/\{sid\}/g, 'SID'),
  );
});

test('the Windows DACL script calls .NET directly and autoloads no PowerShell module', async () => {
  // CI steps run under pwsh 7; its PSModulePath leaks into the powershell.exe
  // 5.1 child, which then cannot autoload Microsoft.PowerShell.Security, so
  // the script must stay cmdlet-free (Get-Acl failed exactly this way).
  const fixture = await readFile(join(process.cwd(), 'desktop-fixture.ts'), 'utf8');
  const script = fixture.slice(
    fixture.indexOf('async function restrictWindowsPrivate('),
    fixture.indexOf('export async function startDesktopFixture('),
  );
  const lines = [...script.matchAll(/^\s*["'](.*)["'],$/gm)].map((match) => match[1]!);
  assert.ok(lines.some((line) => line.includes('[IO.Directory]::SetAccessControl(')));
  assert.ok(lines.some((line) => line.includes('[IO.File]::SetAccessControl(')));
  for (const line of lines) assert.doesNotMatch(line, /\b[A-Z][a-z]+-[A-Z][A-Za-z]+\b/, line);
});

test('index readiness waits for the post-text-pass no-model label, never the idle label', () => {
  const step = FLOW.find((candidate) => candidate.kind === 'waitIndexed');
  assert.ok(step?.kind === 'waitIndexed');
  const ready = new RegExp(step.pattern);
  assert.equal(ready.test('—50 filesWatchingEmbedding model unavailableRetry model load'), true);
  for (const early of ['—50 filesWatchingIndexed', '—50 filesWatchingIndexing…', '—WatchingLocal files', '—Indexed 50 files']) {
    assert.equal(ready.test(early), false, early);
  }
});

test('the provenance jump observes the palette without a state-changing frame activation', async () => {
  const runner = await runnerSource();
  const jump = runner.slice(
    runner.indexOf("case 'jumpToHit':"),
    runner.indexOf("case 'inspectReceipt':"),
  );
  const palette = jump.indexOf('await d.paletteContainerCount()');
  const enter = jump.indexOf("await d.chord('Enter')");
  const reader = jump.indexOf('await d.readerText()');
  assert.ok(palette >= 0 && enter > palette && reader > enter);
  assert.equal(jump.includes('captureFrame'), false);
});

test('current reader flow compares rendered details to the native original receipt', () => {
  const step = FLOW.find((candidate) => candidate.kind === 'inspectReceipt');
  assert.ok(step && step.kind === 'inspectReceipt');
  assert.equal(step.path, 'notes/zettel/node-storage.md');
  const search = FLOW.find((candidate) => candidate.kind === 'search');
  assert.ok(search && search.kind === 'search');
  assert.equal(search.intent, 'Exact');
  assert.equal(FLOW.some((candidate) => /federat|Enterprise|interpretation/.test(candidate.title)), false);
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
