import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import {
  copyFile,
  mkdtemp,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { describe, expect, test } from 'vitest';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const validatorPath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'public-oauth-config.mjs',
);
const valid = {
  MDREADER_GOOGLE_DRIVE_CLIENT_ID:
    '123456789-example.apps.googleusercontent.com',
  MDREADER_DROPBOX_CLIENT_ID: 'dropbox-public-app-key',
  MDREADER_DROPBOX_REDIRECT_URI:
    'http://127.0.0.1:53682/oauth/dropbox',
};

function sha256(value: string): string {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

function runValidator(
  values: Record<string, string | undefined>,
  expected = valid,
  executable = validatorPath,
) {
  return execFileAsync(process.execPath, [executable, 'assert-hashes'], {
    env: {
      PATH: process.env.PATH,
      NODE_ENV: 'test',
      ...values,
      EXPECTED_GOOGLE_DRIVE_CLIENT_ID_SHA256: sha256(
        expected.MDREADER_GOOGLE_DRIVE_CLIENT_ID,
      ),
      EXPECTED_DROPBOX_CLIENT_ID_SHA256: sha256(
        expected.MDREADER_DROPBOX_CLIENT_ID,
      ),
      EXPECTED_DROPBOX_REDIRECT_URI_SHA256: sha256(
        expected.MDREADER_DROPBOX_REDIRECT_URI,
      ),
    },
  });
}

describe('shipping public OAuth configuration', () => {
  test('validates the public inputs at the exact shipping build entrypoint', () => {
    const buildScript = readFileSync(
      resolve(
        repositoryRoot,
        'scripts',
        'release',
        'prepare-direct-release.sh',
      ),
      'utf8',
    );
    expect(buildScript).toContain(
      'node scripts/release/public-oauth-config.mjs assert-hashes',
    );
    expect(
      buildScript.indexOf('public-oauth-config.mjs assert-hashes'),
    ).toBeLessThan(buildScript.indexOf("pnpm --filter '@mdreader/app...'"));
  });

  test('requires one committed hash authority for every shipping OAuth input', async () => {
    const authority = JSON.parse(
      readFileSync(
        resolve(
          repositoryRoot,
          'scripts',
          'release',
          'public-oauth-authority.json',
        ),
        'utf8',
      ),
    ) as Record<string, unknown>;
    expect(authority).toEqual({
      schemaVersion: 1,
      googleDriveClientIdSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
      dropboxClientIdSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
      dropboxRedirectUriSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
    });
    await expect(runValidator(valid, valid)).rejects.toThrow(
      /does not match its committed release authority/i,
    );
  });

  test('accepts only values matching the adjacent committed authority', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-oauth-authority-'));
    const copiedValidator = join(root, 'public-oauth-config.mjs');
    const authority = {
      schemaVersion: 1,
      googleDriveClientIdSha256: sha256(
        valid.MDREADER_GOOGLE_DRIVE_CLIENT_ID,
      ),
      dropboxClientIdSha256: sha256(valid.MDREADER_DROPBOX_CLIENT_ID),
      dropboxRedirectUriSha256: sha256(
        valid.MDREADER_DROPBOX_REDIRECT_URI,
      ),
    };

    try {
      await copyFile(validatorPath, copiedValidator);
      await writeFile(
        join(root, 'public-oauth-authority.json'),
        `${JSON.stringify(authority, null, 2)}\n`,
      );

      await expect(
        runValidator(valid, valid, copiedValidator),
      ).resolves.toMatchObject({ stderr: '' });
      await expect(
        runValidator(
          {
            ...valid,
            MDREADER_DROPBOX_CLIENT_ID: 'replacement-public-app-key',
          },
          valid,
          copiedValidator,
        ),
      ).rejects.toThrow(/does not match its committed release authority/i);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test.each(Object.keys(valid))('rejects missing %s', async (name) => {
    await expect(
      runValidator({ ...valid, [name]: undefined }),
    ).rejects.toThrow(/missing|required/i);
  });

  test('rejects a valid public identifier that differs from the sealed hash', async () => {
    await expect(
      runValidator({
        ...valid,
        MDREADER_DROPBOX_CLIENT_ID: 'replacement-public-app-key',
      }),
    ).rejects.toThrow(/committed public OAuth authority|does not match/i);
  });

  test('rejects a privileged Dropbox callback port even with a self-derived hash', async () => {
    const privilegedPort = {
      ...valid,
      MDREADER_DROPBOX_REDIRECT_URI:
        'http://127.0.0.1:80/oauth/dropbox',
    };

    await expect(
      runValidator(privilegedPort, privilegedPort),
    ).rejects.toThrow(/invalid/i);
  });

  test.each([
    'http://127.000.000.001:53682/oauth/dropbox',
    'http://127.00.0.1:53682/oauth/dropbox',
    'http://127.0.0.01:53682/oauth/dropbox',
  ])(
    'rejects noncanonical IPv4 loopback spelling before it can be attested: %s',
    async (redirectUri) => {
      const noncanonical = {
        ...valid,
        MDREADER_DROPBOX_REDIRECT_URI: redirectUri,
      };

      await expect(runValidator(noncanonical, noncanonical)).rejects.toThrow(
        /invalid/i,
      );
    },
  );

  test('rejects a non-runtime IPv4 loopback host before it can be attested', async () => {
    const nonRuntimeLoopback = {
      ...valid,
      MDREADER_DROPBOX_REDIRECT_URI:
        'http://127.0.0.2:53682/oauth/dropbox',
    };

    await expect(
      runValidator(nonRuntimeLoopback, nonRuntimeLoopback),
    ).rejects.toThrow(/invalid/i);
  });

  test.each([
    {
      ...valid,
      MDREADER_GOOGLE_DRIVE_CLIENT_ID: 'not a Google OAuth client ID',
    },
    {
      ...valid,
      MDREADER_DROPBOX_CLIENT_ID: 'dropbox app key with spaces',
    },
    {
      ...valid,
      MDREADER_DROPBOX_REDIRECT_URI:
        'http://localhost:53682/oauth/dropbox',
    },
    {
      ...valid,
      MDREADER_DROPBOX_REDIRECT_URI:
        'http://127.0.0.1/oauth/dropbox',
    },
  ])('rejects malformed public configuration %#', async (values) => {
    await expect(runValidator(values)).rejects.toThrow(
      /invalid|does not match/i,
    );
  });
});
