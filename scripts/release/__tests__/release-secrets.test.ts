import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, readFileSync, readlinkSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, test } from 'vitest';

import { findReleaseSecretMaterial } from '../secret-scan';

const root = resolve(process.cwd(), '..', '..');
const REVIEWED_NON_SECRET_FIXTURES = new Map([
  [
    'catdb/crates/catdb-policy/tests/fixtures/oidc-private.pem',
    '9ce0986d579c93474e0e5eb6d945b79f2b7d0f176ec6ef4b02c99c7c78ec2f1e',
  ],
]);

function isReviewedNonSecretFixture(path: string, contents: string): boolean {
  const expectedDigest = REVIEWED_NON_SECRET_FIXTURES.get(path);
  return (
    expectedDigest !== undefined &&
    createHash('sha256').update(contents).digest('hex') === expectedDigest
  );
}

function readRepositoryEntry(path: string): string | undefined {
  const absolutePath = resolve(root, path);
  const metadata = lstatSync(absolutePath);

  if (metadata.isSymbolicLink()) {
    return readlinkSync(absolutePath);
  }

  return metadata.isFile() ? readFileSync(absolutePath, 'utf8') : undefined;
}

describe('release secret boundary', () => {
  test('a reviewed test fixture loses its exemption when any byte changes', () => {
    const path = 'catdb/crates/catdb-policy/tests/fixtures/oidc-private.pem';
    const contents = readFileSync(resolve(root, path), 'utf8');

    expect(isReviewedNonSecretFixture(path, contents)).toBe(true);
    expect(isReviewedNonSecretFixture(path, `${contents}\n`)).toBe(false);
  });

  test('detects private-key blocks and literal release credential assignments', () => {
    expect(
      findReleaseSecretMaterial([
        {
          path: 'bad.pem',
          contents: `${['-----BEGIN', 'PRIVATE KEY-----'].join(' ')}\nsecret`,
        },
        {
          path: 'bad.env',
          contents: `MDREADER_UPDATER_PRIVATE_KEY=${'A'.repeat(80)}`,
        },
        {
          path: 'bad-r2.env',
          contents: `MDREADER_R2_SECRET_ACCESS_KEY=${'B'.repeat(40)}`,
        },
      ]),
    ).toEqual([
      expect.objectContaining({ path: 'bad.pem', reason: expect.stringMatching(/private key/i) }),
      expect.objectContaining({ path: 'bad.env', reason: expect.stringMatching(/credential/i) }),
      expect.objectContaining({ path: 'bad-r2.env', reason: expect.stringMatching(/credential/i) }),
    ]);
  });

  test('the complete tracked and pending Task 12 tree contains no unreviewed secret material', () => {
    const paths = execFileSync(
      'git',
      ['ls-files', '--cached', '--others', '--exclude-standard'],
      { cwd: root, encoding: 'utf8' },
    )
      .split('\n')
      .filter(Boolean)
      .filter((path) => existsSync(resolve(root, path)))
      .filter((path) => !path.endsWith('.png') && !path.endsWith('.ico') && !path.endsWith('.icns'));
    const files = paths
      .map((path) => ({
        path,
        contents: readRepositoryEntry(path),
      }))
      .filter(
        (entry): entry is { path: string; contents: string } =>
          entry.contents !== undefined,
      )
      .filter(({ path, contents }) => !isReviewedNonSecretFixture(path, contents));

    expect(findReleaseSecretMaterial(files)).toEqual([]);
  });
});
