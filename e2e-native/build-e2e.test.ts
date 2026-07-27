import assert from 'node:assert/strict';
import { test } from 'node:test';

import { resolveSourceCommit } from './build-e2e.mjs';

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
