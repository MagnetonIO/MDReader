import { createHash, generateKeyPairSync } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { mkdtemp, rename, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, test, vi } from 'vitest';

import {
  uploadGooglePlayClosedBeta,
  uploadTestFlight,
  type ExternalToolRunner,
} from '../mobile-beta-upload';

const PACKAGE_ID = 'dev.mdreader.app';
const ANDROID_BYTES = 'signed Android App Bundle';
const IOS_BYTES = 'signed IPA';
const PKCS8_PRIVATE_KEY_BEGIN = ['-----BEGIN', 'PRIVATE KEY-----'].join(' ');
const PKCS8_PRIVATE_KEY_END = ['-----END', 'PRIVATE KEY-----'].join(' ');
const APPSTORE_PRIVATE_KEY = generateKeyPairSync('ec', {
  namedCurve: 'prime256v1',
}).privateKey.export({ type: 'pkcs8', format: 'pem' });

function artifactIdentity(bytes: string) {
  return {
    expectedBytes: Buffer.byteLength(bytes),
    expectedSha256: createHash('sha256').update(bytes).digest('hex'),
  };
}

async function androidFixture(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-play-upload-'));
  const artifact = join(root, 'MDReader.aab');
  await writeFile(artifact, ANDROID_BYTES);
  return artifact;
}

function serviceAccount(email = 'mdreader-play@example.iam.gserviceaccount.com') {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  return JSON.stringify({
    type: 'service_account',
    client_email: email,
    private_key: privateKey.export({ type: 'pkcs8', format: 'pem' }),
    token_uri: 'https://oauth2.googleapis.com/token',
  });
}

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function appStoreFetcher(
  options: {
    bundleId?: string;
    version?: string;
    buildNumber?: string;
    processingState?: string;
    missingBuildResponses?: number;
    processingBuildResponses?: number;
    existingBuildOnSecondGroupPage?: boolean;
    unexpectedGroupPaginationFilter?: boolean;
  } = {},
) {
  let buildRequests = 0;
  let groupAssigned = false;
  return vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
    const url = new URL(String(input));
    if (url.pathname === '/v1/apps') {
      return json({
        data: [
          {
            type: 'apps',
            id: 'app-123',
            attributes: {
              bundleId: options.bundleId ?? PACKAGE_ID,
            },
          },
        ],
      });
    }
    if (url.pathname === '/v1/builds') {
      buildRequests += 1;
      if (buildRequests <= (options.missingBuildResponses ?? 0)) {
        return json({ data: [], included: [] });
      }
      const processing =
        buildRequests <=
        (options.missingBuildResponses ?? 0) +
          (options.processingBuildResponses ?? 0);
      return json({
        data: [
          {
            type: 'builds',
            id: 'build-123',
            attributes: {
              version: options.buildNumber ?? '41',
              processingState: processing
                ? 'PROCESSING'
                : (options.processingState ?? 'VALID'),
            },
            relationships: {
              preReleaseVersion: {
                data: { type: 'preReleaseVersions', id: 'prerelease-123' },
              },
            },
          },
        ],
        included: [
          {
            type: 'preReleaseVersions',
            id: 'prerelease-123',
            attributes: { version: options.version ?? '1.0.0' },
          },
        ],
      });
    }
    if (
      url.pathname ===
      '/v1/betaGroups/group-reviewed-123/relationships/builds'
    ) {
      if (init?.method === 'POST') {
        groupAssigned = true;
        return new Response(null, { status: 204 });
      }
      if (options.existingBuildOnSecondGroupPage) {
        if (url.searchParams.get('cursor') === 'page-two') {
          return json({
            data: [{ type: 'builds', id: 'build-123' }],
            links: { next: null },
          });
        }
        return json({
          data: [],
          links: {
            next:
              'https://api.appstoreconnect.apple.com/v1/betaGroups/' +
              'group-reviewed-123/relationships/builds?limit=200&cursor=page-two' +
              (options.unexpectedGroupPaginationFilter
                ? '&filter%5Bapp%5D=attacker'
                : ''),
          },
        });
      }
      return json({
        data: groupAssigned
          ? [{ type: 'builds', id: 'build-123' }]
          : [],
      });
    }
    throw new Error(`unexpected App Store Connect request ${url}`);
  });
}

describe('mobile beta provider upload boundaries', () => {
  test('validates and uploads an IPA with App Store Connect API-key authentication', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-upload-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(
      privateKeyPath,
      APPSTORE_PRIVATE_KEY,
      { mode: 0o600 },
    );
    const calls: Array<{ command: string; args: readonly string[] }> = [];
    const runner: ExternalToolRunner = (command, args) => {
      const artifactArgument = args[args.indexOf('-f') + 1]!;
      const keyArgument = args[args.indexOf('--p8-file-path') + 1]!;
      expect(artifactArgument).not.toBe(artifact);
      expect(keyArgument).not.toBe(privateKeyPath);
      expect(readFileSync(artifactArgument, 'utf8')).toBe(IOS_BYTES);
      expect(readFileSync(keyArgument, 'utf8')).toContain(
        PKCS8_PRIVATE_KEY_BEGIN,
      );
      calls.push({ command, args });
      return JSON.stringify({
        success: true,
        deliveryId: 'delivery-1',
        localPath: '/private/runner/MDReader.ipa',
        diagnostic: 'provider-specific upload detail',
      });
    };
    const fetcher = appStoreFetcher();

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        runner,
        fetcher,
        1_753_680_000_000,
      ),
    ).resolves.toEqual({
      submission: { tool: 'altool', completed: true },
      appStoreConnect: {
        appId: 'app-123',
        buildId: 'build-123',
        preReleaseVersionId: 'prerelease-123',
        processingState: 'VALID',
        betaGroupId: 'group-reviewed-123',
        verified: true,
      },
    });

    expect(calls).toHaveLength(2);
    expect(calls[0]!.command).toBe('xcrun');
    expect(calls[0]!.args).toContain('--validate-app');
    expect(calls[1]!.args).toContain('--upload-app');
    expect(JSON.stringify(calls)).not.toContain('BEGIN PRIVATE KEY');
    expect(fetcher).toHaveBeenCalledTimes(5);
  });

  test('follows bounded App Store beta-group pages before deciding to associate', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-pages-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(privateKeyPath, APPSTORE_PRIVATE_KEY, { mode: 0o600 });
    const fetcher = appStoreFetcher({ existingBuildOnSecondGroupPage: true });

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        () => JSON.stringify({ success: true }),
        fetcher,
        1_753_680_000_000,
      ),
    ).resolves.toMatchObject({
      appStoreConnect: {
        buildId: 'build-123',
        betaGroupId: 'group-reviewed-123',
        verified: true,
      },
    });

    expect(
      fetcher.mock.calls.filter(([, init]) => init?.method === 'POST'),
    ).toHaveLength(0);
    expect(fetcher).toHaveBeenCalledTimes(6);
  });

  test('rejects App Store pagination links that add an unexpected query filter', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-filter-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(privateKeyPath, APPSTORE_PRIVATE_KEY, { mode: 0o600 });

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        () => JSON.stringify({ success: true }),
        appStoreFetcher({
          existingBuildOnSecondGroupPage: true,
          unexpectedGroupPaginationFilter: true,
        }),
        1_753_680_000_000,
      ),
    ).rejects.toThrow(/pagination link is not trusted/);
  });

  test('uploads private snapshots when original TestFlight paths are replaced', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-swap-'));
    const artifact = join(root, 'MDReader.ipa');
    const replacement = join(root, 'replacement.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, 'reviewed signed IPA');
    await writeFile(replacement, 'attacker IPA');
    await writeFile(
      privateKeyPath,
      APPSTORE_PRIVATE_KEY,
      { mode: 0o600 },
    );
    const calls: Array<readonly string[]> = [];
    const runner: ExternalToolRunner = (_command, args) => {
      calls.push(args);
      if (calls.length === 1) {
        execFileSync('/bin/mv', ['-f', replacement, artifact]);
        writeFileSync(
          privateKeyPath,
          `${PKCS8_PRIVATE_KEY_BEGIN}\nattacker\n${PKCS8_PRIVATE_KEY_END}\n`,
        );
      }
      return JSON.stringify({ success: true });
    };

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity('reviewed signed IPA'),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        runner,
        appStoreFetcher(),
        1_753_680_000_000,
      ),
    ).resolves.toMatchObject({
      appStoreConnect: { buildId: 'build-123', verified: true },
    });
    expect(calls).toHaveLength(2);
    expect(calls[0]).not.toContain(artifact);
    expect(calls[1]).not.toContain(artifact);
  });

  test('rejects an App Store Connect read-back for a different version', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-readback-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(
      privateKeyPath,
      APPSTORE_PRIVATE_KEY,
      { mode: 0o600 },
    );
    const runner: ExternalToolRunner = () =>
      JSON.stringify({ success: true, deliveryId: 'delivery-1' });

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        runner,
        appStoreFetcher({ version: '9.9.9' }),
        1_753_680_000_000,
      ),
    ).rejects.toThrow(/version/);
  });

  test('polls bounded App Store indexing until the exact build appears', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-poll-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(privateKeyPath, APPSTORE_PRIVATE_KEY, { mode: 0o600 });
    const sleeps: number[] = [];
    const fetcher = appStoreFetcher({
      missingBuildResponses: 2,
      processingBuildResponses: 1,
    });

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        () => JSON.stringify({ success: true }),
        fetcher,
        1_753_680_000_000,
        async (milliseconds) => {
          sleeps.push(milliseconds);
        },
      ),
    ).resolves.toMatchObject({
      appStoreConnect: { buildId: 'build-123', verified: true },
    });
    expect(sleeps).toEqual([1000, 2000, 4000]);
    expect(fetcher).toHaveBeenCalledTimes(8);
  });

  test('rejects an oversized App Store Connect read-back response', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-testflight-bounded-'));
    const artifact = join(root, 'MDReader.ipa');
    const privateKeyPath = join(root, 'AuthKey_ABC123DEF4.p8');
    await writeFile(artifact, IOS_BYTES);
    await writeFile(privateKeyPath, APPSTORE_PRIVATE_KEY, { mode: 0o600 });
    const fetcher = vi.fn(async () => {
      return new Response('{}', {
        status: 200,
        headers: { 'content-length': String(3 * 1024 * 1024) },
      });
    });

    await expect(
      uploadTestFlight(
        {
          artifactPath: artifact,
          ...artifactIdentity(IOS_BYTES),
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedBetaGroupId: 'group-reviewed-123',
          apiKeyId: 'ABC123DEF4',
          issuerId: '11111111-2222-3333-4444-555555555555',
          privateKeyPath,
        },
        () => JSON.stringify({ success: true }),
        fetcher,
        1_753_680_000_000,
      ),
    ).rejects.toThrow(/byte limit/);
  });

  test('uploads only to the reviewed custom closed track and commits the edit', async () => {
    const artifactPath = await androidFixture();
    const requests: Array<{ url: string; init: RequestInit }> = [];
    let editCreations = 0;
    const fetcher = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      requests.push({ url, init: init ?? {} });
      if (url === 'https://oauth2.googleapis.com/token') {
        return json({ access_token: 'short-lived-token', expires_in: 3600 });
      }
      if (url.endsWith('/edits') && init?.method === 'POST') {
        editCreations += 1;
        return json({ id: editCreations === 1 ? 'edit-123' : 'readback-456' });
      }
      if (url.endsWith('/tracks/mdreader-closed-beta')) {
        if (init?.method === 'GET') {
          if (url.includes('readback-456')) {
            return json({
              track: 'mdreader-closed-beta',
              releases: [
                {
                  name: 'MDReader 1.0.0 (41)',
                  versionCodes: ['41'],
                  status: 'completed',
                },
              ],
            });
          }
          return json({ track: 'mdreader-closed-beta' });
        }
        if (init?.method === 'PUT') return json({ track: 'mdreader-closed-beta' });
      }
      if (url.includes('/bundles?uploadType=media')) {
        return json({ versionCode: 41 });
      }
      if (url.endsWith('/edits/edit-123:commit')) {
        return json({ id: 'edit-123', expiryTimeSeconds: '123' });
      }
      if (url.endsWith('/edits/readback-456') && init?.method === 'DELETE') {
        return new Response(null, { status: 204 });
      }
      throw new Error(`unexpected request ${init?.method} ${url}`);
    });

    await expect(
      uploadGooglePlayClosedBeta(
        {
          artifactPath,
          ...artifactIdentity(ANDROID_BYTES),
          serviceAccountJson: serviceAccount(),
          expectedServiceAccountEmail:
            'mdreader-play@example.iam.gserviceaccount.com',
          packageId: PACKAGE_ID,
          closedTrack: 'mdreader-closed-beta',
          versionCode: '41',
          releaseName: 'MDReader 1.0.0 (41)',
        },
        fetcher,
        1_753_680_000_000,
      ),
    ).resolves.toEqual({
      editId: 'edit-123',
      packageId: PACKAGE_ID,
      track: 'mdreader-closed-beta',
      versionCode: '41',
      committed: true,
      providerVerified: true,
    });

    const trackUpdate = requests.find(
      (request) =>
        request.url.endsWith('/tracks/mdreader-closed-beta') &&
        request.init.method === 'PUT',
    )!;
    expect(JSON.parse(String(trackUpdate.init.body))).toEqual({
      track: 'mdreader-closed-beta',
      releases: [
        {
          name: 'MDReader 1.0.0 (41)',
          versionCodes: ['41'],
          status: 'completed',
        },
      ],
    });
    expect(
      requests.some(({ url }) => url.endsWith('/edits/edit-123:commit')),
    ).toBe(true);
    expect(
      requests.some(({ url }) =>
        url.endsWith('/edits/readback-456/tracks/mdreader-closed-beta'),
      ),
    ).toBe(true);
    expect(JSON.stringify(requests)).not.toContain('PRIVATE KEY');
  });

  test('uploads the already-open Android bytes when its path is replaced', async () => {
    const artifactPath = await androidFixture();
    const replacement = `${artifactPath}.replacement`;
    await writeFile(replacement, 'attacker Android App Bundle');
    let uploadedBody: BodyInit | null | undefined;
    let editCreations = 0;
    const fetcher = vi.fn(
      async (input: string | URL | Request, init?: RequestInit) => {
        const url = String(input);
        if (url === 'https://oauth2.googleapis.com/token') {
          await rename(replacement, artifactPath);
          return json({ access_token: 'short-lived-token', expires_in: 3600 });
        }
        if (url.endsWith('/edits') && init?.method === 'POST') {
          editCreations += 1;
          return json({ id: editCreations === 1 ? 'edit-123' : 'readback-456' });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'GET'
        ) {
          if (url.includes('readback-456')) {
            return json({
              track: 'mdreader-closed-beta',
              releases: [
                {
                  name: 'MDReader 1.0.0 (41)',
                  versionCodes: ['41'],
                  status: 'completed',
                },
              ],
            });
          }
          return json({ track: 'mdreader-closed-beta' });
        }
        if (url.includes('/bundles?uploadType=media')) {
          uploadedBody = init?.body;
          return json({ versionCode: 41 });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'PUT'
        ) {
          return json({ track: 'mdreader-closed-beta' });
        }
        if (url.endsWith('/edits/edit-123:commit')) {
          return json({ id: 'edit-123' });
        }
        if (url.endsWith('/edits/readback-456') && init?.method === 'DELETE') {
          return new Response(null, { status: 204 });
        }
        throw new Error(`unexpected request ${init?.method} ${url}`);
      },
    );

    await uploadGooglePlayClosedBeta(
      {
        artifactPath,
        ...artifactIdentity(ANDROID_BYTES),
        serviceAccountJson: serviceAccount(),
        expectedServiceAccountEmail:
          'mdreader-play@example.iam.gserviceaccount.com',
        packageId: PACKAGE_ID,
        closedTrack: 'mdreader-closed-beta',
        versionCode: '41',
        releaseName: 'MDReader 1.0.0 (41)',
      },
      fetcher,
      1_753_680_000_000,
    );

    expect(Buffer.from(uploadedBody as Uint8Array).toString('utf8')).toBe(
      'signed Android App Bundle',
    );
  });

  test('rejects a committed Google Play release absent from fresh track read-back', async () => {
    let editCreations = 0;
    const fetcher = vi.fn(
      async (input: string | URL | Request, init?: RequestInit) => {
        const url = String(input);
        if (url === 'https://oauth2.googleapis.com/token') {
          return json({ access_token: 'short-lived-token' });
        }
        if (url.endsWith('/edits') && init?.method === 'POST') {
          editCreations += 1;
          return json({ id: editCreations === 1 ? 'edit-123' : 'readback-456' });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'GET'
        ) {
          return url.includes('readback-456')
            ? json({
                track: 'mdreader-closed-beta',
                releases: [
                  {
                    name: 'old release',
                    versionCodes: ['40'],
                    status: 'completed',
                  },
                ],
              })
            : json({ track: 'mdreader-closed-beta' });
        }
        if (url.includes('/bundles?uploadType=media')) {
          return json({ versionCode: 41 });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'PUT'
        ) {
          return json({ track: 'mdreader-closed-beta' });
        }
        if (url.endsWith('/edits/edit-123:commit')) return json({ id: 'edit-123' });
        if (url.endsWith('/edits/readback-456') && init?.method === 'DELETE') {
          return new Response(null, { status: 500 });
        }
        throw new Error(`unexpected request ${init?.method} ${url}`);
      },
    );

    await expect(
      uploadGooglePlayClosedBeta(
        {
          artifactPath: await androidFixture(),
          ...artifactIdentity(ANDROID_BYTES),
          serviceAccountJson: serviceAccount(),
          expectedServiceAccountEmail:
            'mdreader-play@example.iam.gserviceaccount.com',
          packageId: PACKAGE_ID,
          closedTrack: 'mdreader-closed-beta',
          versionCode: '41',
          releaseName: 'MDReader 1.0.0 (41)',
        },
        fetcher,
      ),
    ).rejects.toThrow(/read-back does not match/);
  });

  test('fails when a matching Google Play read-back edit cannot be cleaned up', async () => {
    let editCreations = 0;
    const fetcher = vi.fn(
      async (input: string | URL | Request, init?: RequestInit) => {
        const url = String(input);
        if (url === 'https://oauth2.googleapis.com/token') {
          return json({ access_token: 'short-lived-token' });
        }
        if (url.endsWith('/edits') && init?.method === 'POST') {
          editCreations += 1;
          return json({
            id: editCreations === 1 ? 'edit-123' : 'readback-456',
          });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'GET'
        ) {
          return url.includes('readback-456')
            ? json({
                track: 'mdreader-closed-beta',
                releases: [
                  {
                    name: 'MDReader 1.0.0 (41)',
                    versionCodes: ['41'],
                    status: 'completed',
                  },
                ],
              })
            : json({ track: 'mdreader-closed-beta' });
        }
        if (url.includes('/bundles?uploadType=media')) {
          return json({ versionCode: 41 });
        }
        if (
          url.endsWith('/tracks/mdreader-closed-beta') &&
          init?.method === 'PUT'
        ) {
          return json({ track: 'mdreader-closed-beta' });
        }
        if (url.endsWith('/edits/edit-123:commit')) {
          return json({ id: 'edit-123' });
        }
        if (
          url.endsWith('/edits/readback-456') &&
          init?.method === 'DELETE'
        ) {
          return new Response(null, { status: 500 });
        }
        throw new Error(`unexpected request ${init?.method} ${url}`);
      },
    );

    await expect(
      uploadGooglePlayClosedBeta(
        {
          artifactPath: await androidFixture(),
          ...artifactIdentity(ANDROID_BYTES),
          serviceAccountJson: serviceAccount(),
          expectedServiceAccountEmail:
            'mdreader-play@example.iam.gserviceaccount.com',
          packageId: PACKAGE_ID,
          closedTrack: 'mdreader-closed-beta',
          versionCode: '41',
          releaseName: 'MDReader 1.0.0 (41)',
        },
        fetcher,
      ),
    ).rejects.toThrow(/read-back edit cleanup failed/);
  });

  test.each(['production', 'beta', 'alpha', 'qa', 'internal'])(
    'refuses reserved Google Play track %s before authentication',
    async (closedTrack) => {
      const fetcher = vi.fn();
      await expect(
        uploadGooglePlayClosedBeta(
          {
            artifactPath: await androidFixture(),
            ...artifactIdentity(ANDROID_BYTES),
            serviceAccountJson: serviceAccount(),
            expectedServiceAccountEmail:
              'mdreader-play@example.iam.gserviceaccount.com',
            packageId: PACKAGE_ID,
            closedTrack,
            versionCode: '41',
            releaseName: 'MDReader 1.0.0 (41)',
          },
          fetcher,
        ),
      ).rejects.toThrow(/closed track/);
      expect(fetcher).not.toHaveBeenCalled();
    },
  );

  test('refuses a service-account identity that differs from reviewed non-secret authority', async () => {
    const fetcher = vi.fn();
    await expect(
      uploadGooglePlayClosedBeta(
        {
          artifactPath: await androidFixture(),
          ...artifactIdentity(ANDROID_BYTES),
          serviceAccountJson: serviceAccount('attacker@example.iam.gserviceaccount.com'),
          expectedServiceAccountEmail:
            'mdreader-play@example.iam.gserviceaccount.com',
          packageId: PACKAGE_ID,
          closedTrack: 'mdreader-closed-beta',
          versionCode: '41',
          releaseName: 'MDReader 1.0.0 (41)',
        },
        fetcher,
      ),
    ).rejects.toThrow(/service account/);
    expect(fetcher).not.toHaveBeenCalled();
  });

  test('deletes an uncommitted edit after a provider failure', async () => {
    const requests: Array<{ url: string; method: string }> = [];
    const fetcher = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      requests.push({ url, method });
      if (url === 'https://oauth2.googleapis.com/token') {
        return json({ access_token: 'token', expires_in: 3600 });
      }
      if (url.endsWith('/edits') && method === 'POST') {
        return json({ id: 'edit-failed' });
      }
      if (url.endsWith('/tracks/mdreader-closed-beta') && method === 'GET') {
        return json({ track: 'mdreader-closed-beta' });
      }
      if (url.includes('/bundles?uploadType=media')) {
        return json({ error: { message: 'rejected' } }, 400);
      }
      if (url.endsWith('/edits/edit-failed') && method === 'DELETE') {
        return new Response(null, { status: 204 });
      }
      throw new Error(`unexpected request ${method} ${url}`);
    });

    await expect(
      uploadGooglePlayClosedBeta(
        {
          artifactPath: await androidFixture(),
          ...artifactIdentity(ANDROID_BYTES),
          serviceAccountJson: serviceAccount(),
          expectedServiceAccountEmail:
            'mdreader-play@example.iam.gserviceaccount.com',
          packageId: PACKAGE_ID,
          closedTrack: 'mdreader-closed-beta',
          versionCode: '41',
          releaseName: 'MDReader 1.0.0 (41)',
        },
        fetcher,
      ),
    ).rejects.toThrow(/App Bundle upload failed/);
    expect(requests.at(-1)).toEqual({
      url: `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${PACKAGE_ID}/edits/edit-failed`,
      method: 'DELETE',
    });
    expect(requests.some(({ url }) => url.endsWith(':commit'))).toBe(false);
  });
});
