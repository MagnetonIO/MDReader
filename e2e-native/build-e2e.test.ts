import assert from 'node:assert/strict';
import { test } from 'node:test';

import { pnpmInvocation, resolveSourceCommit } from './build-e2e.mjs';

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
