import { createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, test } from 'vitest';

import {
  createMobileBetaUploadReceipt,
  readMobileBetaUploadManifest,
  runWithReservedMobileBetaReceipt,
} from '../mobile-beta-cli';
import type { MobileBetaManifest } from '../mobile-beta-manifest';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const ARTIFACT_SHA256 = 'ab'.repeat(32);

function iosManifest(
  fileName = 'MDReader-1.0.0-41.retained.ipa',
): MobileBetaManifest {
  return {
    schemaVersion: 2,
    platform: 'ios',
    channel: 'testflight',
    sourceCommit: SOURCE_COMMIT,
    version: '1.0.0',
    buildNumber: '41',
    packageId: 'dev.mdreader.app',
    architectures: ['arm64'],
    signingIdentity: {
      kind: 'apple-distribution',
      teamId: '7727LYTG96',
      certificateSha256: 'cd'.repeat(32),
    },
    iosAuthority: {
      provisioningProfileUuid: '11111111-2222-3333-4444-555555555555',
      provisioningProfileSha256: '11'.repeat(32),
      profileEntitlementsSha256: '22'.repeat(32),
      signedEntitlementsSha256: '33'.repeat(32),
    },
    artifact: {
      fileName,
      bytes: 123,
      sha256: ARTIFACT_SHA256,
    },
  };
}

describe('mobile beta CLI publication binding', () => {
  test('derives the only uploadable artifact from the protected manifest', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-cli-'));
    const manifestPath = join(root, 'mobile-beta-manifest.json');
    const body = `${JSON.stringify(iosManifest(), null, 2)}\n`;
    await writeFile(manifestPath, body, { mode: 0o600 });

    const loaded = await readMobileBetaUploadManifest(manifestPath);
    expect(loaded.manifest.artifactPath).toBe(
      join(root, 'MDReader-1.0.0-41.retained.ipa'),
    );
    expect(loaded.sha256).toBe(
      createHash('sha256').update(body).digest('hex'),
    );
  });

  test('binds provider evidence to manifest, source, version, build, and artifact digest', () => {
    const manifest = iosManifest();
    expect(
      createMobileBetaUploadReceipt(
        '/private/release/mobile-beta-manifest.json',
        'ef'.repeat(32),
        manifest,
        {
          submission: { tool: 'altool', completed: true },
          appStoreConnect: {
            appId: 'app-123',
            buildId: 'build-123',
            preReleaseVersionId: 'prerelease-123',
            processingState: 'VALID',
            betaGroupId: 'group-reviewed-123',
            verified: true,
          },
        },
      ),
    ).toEqual({
      schemaVersion: 2,
      status: 'succeeded',
      platform: 'ios',
      channel: 'testflight',
      sourceCommit: SOURCE_COMMIT,
      version: '1.0.0',
      buildNumber: '41',
      packageId: 'dev.mdreader.app',
      artifact: {
        fileName: 'MDReader-1.0.0-41.retained.ipa',
        bytes: 123,
        sha256: ARTIFACT_SHA256,
      },
      manifest: {
        fileName: 'mobile-beta-manifest.json',
        sha256: 'ef'.repeat(32),
      },
      providerReceipt: {
        submission: { tool: 'altool', completed: true },
        appStoreConnect: {
          appId: 'app-123',
          buildId: 'build-123',
          preReleaseVersionId: 'prerelease-123',
          processingState: 'VALID',
          betaGroupId: 'group-reviewed-123',
          verified: true,
        },
      },
    });
  });

  test('refuses raw provider output and local paths in a durable receipt', () => {
    expect(() =>
      createMobileBetaUploadReceipt(
        '/private/release/mobile-beta-manifest.json',
        'ef'.repeat(32),
        iosManifest(),
        {
          providerUpload: {
            success: true,
            localPath: '/private/runner/MDReader.ipa',
          },
          appStoreConnect: {
            buildId: 'build-123',
            verified: true,
          },
        },
      ),
    ).toThrow(/provider receipt/i);
  });

  test('reserves the receipt before provider effects and records ambiguous failure durably', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-receipt-'));
    const collision = join(root, 'existing-receipt.json');
    await writeFile(collision, 'already owned', { mode: 0o600 });
    let called = false;
    await expect(
      runWithReservedMobileBetaReceipt(
        collision,
        join(root, 'mobile-beta-manifest.json'),
        'ef'.repeat(32),
        iosManifest(),
        async () => {
          called = true;
          return { uploaded: true };
        },
      ),
    ).rejects.toThrow();
    expect(called).toBe(false);

    const failurePath = join(root, 'failed-receipt.json');
    let reservationObserved = false;
    await expect(
      runWithReservedMobileBetaReceipt(
        failurePath,
        join(root, 'mobile-beta-manifest.json'),
        'ef'.repeat(32),
        iosManifest(),
        async () => {
          expect(JSON.parse(await readFile(failurePath, 'utf8'))).toMatchObject({
            status: 'provider-outcome-unknown',
            sourceCommit: SOURCE_COMMIT,
            version: '1.0.0',
            buildNumber: '41',
            artifact: { sha256: ARTIFACT_SHA256 },
            manifest: { sha256: 'ef'.repeat(32) },
            providerReceipt: null,
          });
          reservationObserved = true;
          throw new Error('provider commit outcome unknown');
        },
      ),
    ).rejects.toThrow(/outcome unknown/);
    expect(reservationObserved).toBe(true);
    expect(JSON.parse(await readFile(failurePath, 'utf8'))).toMatchObject({
      status: 'failed-or-ambiguous',
      sourceCommit: SOURCE_COMMIT,
      version: '1.0.0',
      buildNumber: '41',
      artifact: { sha256: ARTIFACT_SHA256 },
      manifest: { sha256: 'ef'.repeat(32) },
      failure: {
        kind: 'provider-operation-failed-or-ambiguous',
      },
    });
  });

  test('rejects a manifest that tries to redirect upload outside its directory', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-cli-escape-'));
    const manifestPath = join(root, 'mobile-beta-manifest.json');
    await writeFile(
      manifestPath,
      `${JSON.stringify(iosManifest('../attacker.ipa'))}\n`,
      { mode: 0o600 },
    );
    await expect(readMobileBetaUploadManifest(manifestPath)).rejects.toThrow(
      /malformed|production-authorized/,
    );
  });
});
