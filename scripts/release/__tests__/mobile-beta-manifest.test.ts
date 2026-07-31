import {
  mkdtemp,
  open,
  readFile,
  rename,
  stat,
  utimes,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, test } from 'vitest';

import {
  createMobileBetaManifest,
  retainMobileBetaArtifact,
  snapshotOpenedMobileBetaArtifact,
  writeMobileBetaManifest,
  type MobileBetaArtifactEvidence,
} from '../mobile-beta-manifest';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const IOS_CERTIFICATE = '1'.repeat(64);
const ANDROID_CERTIFICATE = '2'.repeat(64);
const IOS_AUTHORITY = {
  provisioningProfileUuid: '11111111-2222-3333-4444-555555555555',
  provisioningProfileSha256: '3'.repeat(64),
  profileEntitlementsSha256: '4'.repeat(64),
  signedEntitlementsSha256: '5'.repeat(64),
};

async function fixture(name: string, body = 'signed package bytes') {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-beta-manifest-'));
  const artifactPath = join(root, name);
  await writeFile(artifactPath, body);
  return { root, artifactPath };
}

function iosEvidence(artifactPath: string): MobileBetaArtifactEvidence {
  return {
    platform: 'ios',
    channel: 'testflight',
    artifactPath,
    sourceCommit: SOURCE_COMMIT,
    version: '1.0.0',
    buildNumber: '41',
    packageId: 'dev.mdreader.app',
    architectures: ['arm64'],
    signingIdentity: {
      kind: 'apple-distribution',
      teamId: '7727LYTG96',
      certificateSha256: IOS_CERTIFICATE,
    },
    iosAuthority: IOS_AUTHORITY,
  };
}

function androidEvidence(artifactPath: string): MobileBetaArtifactEvidence {
  return {
    platform: 'android',
    channel: 'google-play-closed',
    artifactPath,
    sourceCommit: SOURCE_COMMIT,
    version: '1.0.0',
    buildNumber: '41',
    packageId: 'dev.mdreader.app',
    architectures: ['arm64-v8a'],
    signingIdentity: {
      kind: 'google-play-upload',
      certificateSha256: ANDROID_CERTIFICATE,
    },
  };
}

describe('mobile beta artifact manifest', () => {
  test('binds a verified production package to source, version, architecture, signer, bytes, and channel', async () => {
    const ios = await fixture('MDReader.ipa');
    const manifest = await createMobileBetaManifest(iosEvidence(ios.artifactPath), {
      expectedSourceCommit: SOURCE_COMMIT,
      expectedVersion: '1.0.0',
      expectedBuildNumber: '41',
      expectedAppleTeamId: '7727LYTG96',
      expectedSigningCertificateSha256: IOS_CERTIFICATE,
    });

    expect(manifest).toEqual({
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
        certificateSha256: IOS_CERTIFICATE,
      },
      iosAuthority: IOS_AUTHORITY,
      artifact: {
        fileName: 'MDReader.ipa',
        bytes: 20,
        sha256:
          '27c85cdfadf02501e7ecf62a50f150163f2be17e2d8cec65b400060819986fb2',
      },
    });

    const android = await fixture('MDReader.aab', 'android signed package');
    const androidManifest = await createMobileBetaManifest(
      androidEvidence(android.artifactPath),
      {
        expectedSourceCommit: SOURCE_COMMIT,
        expectedVersion: '1.0.0',
        expectedBuildNumber: '41',
        expectedSigningCertificateSha256: ANDROID_CERTIFICATE,
        expectedGooglePlayTrack: 'mdreader-closed-beta',
      },
    );
    expect(androidManifest.platform).toBe('android');
    expect(androidManifest.channel).toBe('google-play-closed');
    expect(androidManifest.architectures).toEqual(['arm64-v8a']);
    expect(androidManifest.artifact.fileName).toBe('MDReader.aab');
    expect(androidManifest.androidAuthority).toEqual({
      googlePlayTrack: 'mdreader-closed-beta',
    });
  });

  test.each([
    ['E2E package identity', { packageId: 'dev.mdreader.app.e2e' }],
    ['wrong source', { sourceCommit: 'f'.repeat(40) }],
    ['wrong version', { version: '1.0.1' }],
    ['wrong build number', { buildNumber: '42' }],
    ['simulator architecture', { architectures: ['arm64-sim'] }],
    [
      'unapproved signer',
      {
        signingIdentity: {
          kind: 'apple-distribution',
          teamId: '7727LYTG96',
          certificateSha256: 'f'.repeat(64),
        },
      },
    ],
  ])('refuses %s instead of publishing a candidate', async (_name, mutation) => {
    const { artifactPath } = await fixture('MDReader.ipa');
    await expect(
      createMobileBetaManifest(
        { ...iosEvidence(artifactPath), ...mutation } as MobileBetaArtifactEvidence,
        {
          expectedSourceCommit: SOURCE_COMMIT,
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedAppleTeamId: '7727LYTG96',
          expectedSigningCertificateSha256: IOS_CERTIFICATE,
        },
      ),
    ).rejects.toThrow();
  });

  test('refuses Android APKs and extra ABIs', async () => {
    const apk = await fixture('MDReader.apk');
    await expect(
      createMobileBetaManifest(androidEvidence(apk.artifactPath), {
        expectedSourceCommit: SOURCE_COMMIT,
        expectedVersion: '1.0.0',
        expectedBuildNumber: '41',
        expectedSigningCertificateSha256: ANDROID_CERTIFICATE,
      }),
    ).rejects.toThrow(/AAB/);

    const aab = await fixture('MDReader.aab');
    await expect(
      createMobileBetaManifest(
        {
          ...androidEvidence(aab.artifactPath),
          architectures: ['arm64-v8a', 'x86_64'],
        },
        {
          expectedSourceCommit: SOURCE_COMMIT,
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedSigningCertificateSha256: ANDROID_CERTIFICATE,
          expectedGooglePlayTrack: 'mdreader-closed-beta',
        },
      ),
    ).rejects.toThrow(/architecture/);
  });

  test('writes once and refuses to replace an immutable manifest', async () => {
    const { root, artifactPath } = await fixture('MDReader.ipa');
    const output = join(root, 'mobile-beta-manifest.json');
    const manifest = await createMobileBetaManifest(
      iosEvidence(artifactPath),
      {
        expectedSourceCommit: SOURCE_COMMIT,
        expectedVersion: '1.0.0',
        expectedBuildNumber: '41',
        expectedAppleTeamId: '7727LYTG96',
        expectedSigningCertificateSha256: IOS_CERTIFICATE,
      },
    );

    await writeMobileBetaManifest(output, manifest);
    expect(JSON.parse(await readFile(output, 'utf8'))).toEqual(manifest);
    await expect(writeMobileBetaManifest(output, manifest)).rejects.toThrow();
  });

  test('snapshots and hashes the same opened descriptor when the source path is replaced', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-snapshot-race-'));
    const source = join(root, 'candidate.ipa');
    const replacement = join(root, 'replacement.ipa');
    const snapshot = join(root, 'retained.ipa');
    await writeFile(source, 'reviewed candidate bytes');
    await writeFile(replacement, 'different attacker bytes');
    const sourceHandle = await open(source, 'r');
    const openedIdentity = await sourceHandle.stat();
    await rename(replacement, source);

    const retained = await snapshotOpenedMobileBetaArtifact(
      sourceHandle,
      openedIdentity,
      snapshot,
    );
    await sourceHandle.close();

    expect(await readFile(snapshot, 'utf8')).toBe('reviewed candidate bytes');
    expect(await readFile(source, 'utf8')).toBe('different attacker bytes');
    expect(retained).toEqual({
      path: snapshot,
      bytes: 24,
      sha256:
        'aed36a915265b80fa9b72773368f46b0773b468aa7103d54fd2aa56ff29fb3a6',
    });
    expect((await stat(snapshot)).mode & 0o077).toBe(0);
  });

  test('builds the manifest from the inspected descriptor when its retained path is replaced', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-manifest-race-'));
    const artifactPath = join(root, 'MDReader.ipa');
    const replacement = join(root, 'replacement.ipa');
    await writeFile(artifactPath, 'reviewed package bytes');
    await writeFile(replacement, 'attacker package bytes');
    const inspected = await open(artifactPath, 'r');
    const inspectedIdentity = await inspected.stat();
    await rename(replacement, artifactPath);

    const manifest = await createMobileBetaManifest(
      iosEvidence(artifactPath),
      {
        expectedSourceCommit: SOURCE_COMMIT,
        expectedVersion: '1.0.0',
        expectedBuildNumber: '41',
        expectedAppleTeamId: '7727LYTG96',
        expectedSigningCertificateSha256: IOS_CERTIFICATE,
      },
      { handle: inspected, identity: inspectedIdentity },
    );
    await inspected.close();

    expect(manifest.artifact).toEqual({
      fileName: 'MDReader.ipa',
      bytes: 22,
      sha256:
        '9074a406093263d77bf04f19cc0bd22fe1d11aa475a3c88a2613edc1482d9f7f',
    });
    expect(await readFile(artifactPath, 'utf8')).toBe('attacker package bytes');
  });

  test('keeps inspection bytes unreachable through the rewritten retained pathname', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-unlinked-race-'));
    const source = join(root, 'candidate.ipa');
    const snapshot = join(root, 'retained.ipa');
    await writeFile(source, 'reviewed-artifact');

    const retained = await retainMobileBetaArtifact(source, snapshot);
    try {
      const visibleIdentity = await stat(snapshot);
      await writeFile(snapshot, 'attacker-artifact');

      const inspected = Buffer.alloc(retained.openedArtifact.identity.size);
      const read = await retained.openedArtifact.handle.read(
        inspected,
        0,
        inspected.byteLength,
        0,
      );
      expect(inspected.subarray(0, read.bytesRead).toString('utf8')).toBe(
        'reviewed-artifact',
      );
      await expect(
        retained.openedArtifact.handle.write(
          Buffer.from('attacker-artifact'),
          0,
          inspected.byteLength,
          0,
        ),
      ).rejects.toThrow();

      await writeFile(snapshot, 'reviewed-artifact');
      await utimes(
        snapshot,
        visibleIdentity.atimeMs / 1000,
        visibleIdentity.mtimeMs / 1000,
      );
      const manifest = await createMobileBetaManifest(
        iosEvidence(snapshot),
        {
          expectedSourceCommit: SOURCE_COMMIT,
          expectedVersion: '1.0.0',
          expectedBuildNumber: '41',
          expectedAppleTeamId: '7727LYTG96',
          expectedSigningCertificateSha256: IOS_CERTIFICATE,
        },
        retained.openedArtifact,
      );

      expect(manifest.artifact).toEqual({
        fileName: 'retained.ipa',
        bytes: 17,
        sha256:
          'ce9a85782130032ebd40623d28e3107f96fce7500fa99a2d721587d8acb8ef2f',
      });
    } finally {
      await retained.openedArtifact.handle.close();
    }
  });
});
