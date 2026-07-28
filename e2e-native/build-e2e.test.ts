import assert from 'node:assert/strict';
import {
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  pnpmInvocation,
  resolveSourceCommit,
  stageMobileSourceCommit,
} from './build-e2e.mjs';
import {
  mobileBuildArgs,
  runMobileBuild,
} from './build-mobile-e2e.mjs';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';

test('E2E builds bind an explicit reviewed source commit', () => {
  assert.equal(
    resolveSourceCommit(SOURCE_COMMIT, () => {
      throw new Error('must not read Git when the caller supplied a commit');
    }),
    SOURCE_COMMIT,
  );
  assert.throws(
    () => resolveSourceCommit('UNBOUND', () => SOURCE_COMMIT),
    /lowercase 40-character Git commit/,
  );
});

test('local E2E builds resolve and validate the current checkout commit', () => {
  assert.equal(resolveSourceCommit(undefined, () => SOURCE_COMMIT), SOURCE_COMMIT);
  assert.throws(
    () => resolveSourceCommit(undefined, () => 'not-a-commit'),
    /lowercase 40-character Git commit/,
  );
});

test('Windows invokes pnpm through its command processor without shell lookup', () => {
  assert.deepEqual(
    pnpmInvocation(
      ['--filter', '@mdreader/app', 'tauri', 'build'],
      'win32',
      'C:\\Windows\\System32\\cmd.exe',
    ),
    {
      command: 'C:\\Windows\\System32\\cmd.exe',
      args: ['/d', '/s', '/c', 'pnpm --filter @mdreader/app tauri build'],
    },
  );
  assert.throws(
    () => pnpmInvocation(['build', '&', 'whoami'], 'win32', 'cmd.exe'),
    /unsafe pnpm argument/,
  );
});

test('mobile builds stage and remove an exact source identity', () => {
  const directory = mkdtempSync(join(tmpdir(), 'mdreader-mobile-identity-'));
  const destination = join(directory, '.e2e-source-commit');
  try {
    const cleanup = stageMobileSourceCommit(SOURCE_COMMIT, destination);

    assert.equal(readFileSync(destination, 'utf8'), `${SOURCE_COMMIT}\n`);
    cleanup();
    assert.throws(() => readFileSync(destination, 'utf8'), /ENOENT/);
    assert.throws(
      () => stageMobileSourceCommit('UNBOUND', destination),
      /lowercase 40-character Git commit/,
    );

    writeFileSync(destination, `${SOURCE_COMMIT}\n`);
    assert.throws(
      () => stageMobileSourceCommit(SOURCE_COMMIT, destination),
      /refusing to replace an existing mobile source-identity handoff/,
    );
    rmSync(destination);

    const changedCleanup = stageMobileSourceCommit(SOURCE_COMMIT, destination);
    writeFileSync(destination, 'changed\n');
    assert.throws(
      changedCleanup,
      /refusing to remove a changed mobile source-identity handoff/,
    );
  } finally {
    rmSync(directory, { force: true, recursive: true });
  }
});

test('mobile build commands preserve the isolated E2E identity contract', () => {
  assert.deepEqual(mobileBuildArgs('ios').slice(-4), [
    'src-tauri/tauri.e2e.conf.json',
    '--no-sign',
    '--archive-only',
    '--ci',
  ]);
  assert.ok(mobileBuildArgs('ios').includes('e2e-build'));
  assert.ok(mobileBuildArgs('android').includes('e2e-build'));
  assert.ok(mobileBuildArgs('android').includes('x86_64'));
  assert.throws(() => mobileBuildArgs('windows'), /unsupported mobile/);
});

test('mobile wrapper cleans the handoff and preserves child exit status', () => {
  const calls = [];
  let cleanupCount = 0;
  const status = runMobileBuild('ios', SOURCE_COMMIT, {
    stage(sourceCommit) {
      assert.equal(sourceCommit, SOURCE_COMMIT);
      return () => {
        cleanupCount += 1;
      };
    },
    run(args, sourceCommit) {
      assert.equal(sourceCommit, SOURCE_COMMIT);
      calls.push(args);
      return calls.length === 1 ? 0 : 23;
    },
  });

  assert.equal(status, 23);
  assert.equal(cleanupCount, 1);
  assert.deepEqual(calls[0], [
    '--filter',
    '@mdreader/app...',
    'run',
    'build',
  ]);
  assert.deepEqual(calls[1], mobileBuildArgs('ios'));
});

test('mobile wrapper does not build an app after a dependency build failure', () => {
  let callCount = 0;
  let cleaned = false;
  const status = runMobileBuild('android', SOURCE_COMMIT, {
    stage() {
      return () => {
        cleaned = true;
      };
    },
    run() {
      callCount += 1;
      return 17;
    },
  });

  assert.equal(status, 17);
  assert.equal(callCount, 1);
  assert.equal(cleaned, true);
});

test('mobile wrapper preserves a child failure when cleanup also fails', () => {
  const cleanupErrors = [];
  const cleanupFailure = new Error('handoff changed');
  const status = runMobileBuild('android', SOURCE_COMMIT, {
    stage() {
      return () => {
        throw cleanupFailure;
      };
    },
    run() {
      return 29;
    },
    reportCleanupError(error) {
      cleanupErrors.push(error);
    },
  });

  assert.equal(status, 29);
  assert.deepEqual(cleanupErrors, [cleanupFailure]);
});

test('mobile wrapper keeps cleanup failure fatal after a successful build', () => {
  const cleanupFailure = new Error('handoff missing');
  assert.throws(
    () =>
      runMobileBuild('ios', SOURCE_COMMIT, {
        stage() {
          return () => {
            throw cleanupFailure;
          };
        },
        run() {
          return 0;
        },
      }),
    cleanupFailure,
  );
});
