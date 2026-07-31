import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import {
  cp,
  lstat,
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, relative, resolve, sep } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, describe, expect, test } from 'vitest';

import { stagePlatformArtifacts } from '../stage-release';
import {
  attestReleaseBuildConfig,
  sealReleaseBundle,
} from '../release-build-attestation';
import { createReleaseConfig } from '../release-config';
import { verifyPackageBinarySet } from '../binary-identity';
import { reviewedExecutable } from './release-binary-fixtures';

const roots: string[] = [];
const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const SIGNATURE = 'U'.repeat(88);
const UPDATER_PUBLIC_KEY = 'V'.repeat(88);
const UPDATER_PUBLIC_KEY_SHA256 =
  '779cae93524f9727af3450fc1ee6b915acb214095046015eaa0108c2612a1634';

const syftFixture = async (artifactPath: string) => {
  const filename = basename(artifactPath);
  const checksum = createHash('sha256')
    .update(await readFile(artifactPath))
    .digest('hex');
  return JSON.stringify({
    spdxVersion: 'SPDX-2.3',
    dataLicense: 'CC0-1.0',
    SPDXID: 'SPDXRef-DOCUMENT',
    name: 'Syft scan',
    documentNamespace: 'https://example.invalid/syft',
    creationInfo: {
      created: '2026-07-26T12:00:00.000Z',
      creators: ['Tool: syft-1.29.0'],
    },
    packages: [
      {
        SPDXID: `SPDXRef-DocumentRoot-File-${filename}`,
        name: filename,
        checksums: [{ algorithm: 'SHA256', checksumValue: checksum }],
      },
    ],
    relationships: [
      {
        spdxElementId: 'SPDXRef-DOCUMENT',
        relationshipType: 'DESCRIBES',
        relatedSpdxElement: `SPDXRef-DocumentRoot-File-${filename}`,
      },
    ],
  });
};

async function linuxBundle() {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-bundle-'));
  const output = await mkdtemp(join(tmpdir(), 'mdreader-stage-'));
  const metadata = await mkdtemp(join(tmpdir(), 'mdreader-release-metadata-'));
  roots.push(root, output, metadata);
  await mkdir(join(root, 'appimage'), { recursive: true });
  await mkdir(join(root, 'deb'), { recursive: true });
  await mkdir(join(root, 'rpm'), { recursive: true });
  await writeFile(join(root, 'appimage', 'MDReader_1.0.0_amd64.AppImage'), 'appimage');
  await writeFile(
    join(root, 'appimage', 'MDReader_1.0.0_amd64.AppImage.tar.gz'),
    'updater',
  );
  await writeFile(
    join(root, 'appimage', 'MDReader_1.0.0_amd64.AppImage.tar.gz.sig'),
    SIGNATURE,
  );
  await writeFile(join(root, 'deb', 'MDReader_1.0.0_amd64.deb'), 'deb');
  await writeFile(join(root, 'rpm', 'MDReader-1.0.0-1.x86_64.rpm'), 'rpm');
  const releaseConfigPath = join(metadata, 'release-config.json');
  const buildAttestationPath = join(metadata, 'build-attestation.json');
  const bundleSealPath = join(metadata, 'bundle-seal.json');
  await writeFile(
    releaseConfigPath,
    `${JSON.stringify(
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: UPDATER_PUBLIC_KEY,
        updaterEndpoint:
          'https://downloads.mdreader.dev/channels/stable/updater.json',
      }),
      null,
      2,
    )}\n`,
  );
  await attestReleaseBuildConfig(
    releaseConfigPath,
    buildAttestationPath,
    'x86_64-unknown-linux-gnu',
    'appimage,deb,rpm',
  );
  await sealReleaseBundle(
    root,
    releaseConfigPath,
    buildAttestationPath,
    bundleSealPath,
    {
      version: '1.0.0',
      target: 'x86_64-unknown-linux-gnu',
      bundles: 'appimage,deb,rpm',
    },
  );
  return {
    root,
    output,
    releaseConfigPath,
    buildAttestationPath,
    bundleSealPath,
  };
}

function attestedBuild(
  fixture: Awaited<ReturnType<typeof linuxBundle>>,
) {
  return {
    releaseConfigPath: fixture.releaseConfigPath,
    buildAttestationPath: fixture.buildAttestationPath,
    bundleSealPath: fixture.bundleSealPath,
    buildTarget: 'x86_64-unknown-linux-gnu',
    buildBundles: 'appimage,deb,rpm',
  };
}

async function reseal(
  fixture: Awaited<ReturnType<typeof linuxBundle>>,
): Promise<void> {
  await sealReleaseBundle(
    fixture.root,
    fixture.releaseConfigPath,
    fixture.buildAttestationPath,
    fixture.bundleSealPath,
    {
      version: '1.0.0',
      target: 'x86_64-unknown-linux-gnu',
      bundles: 'appimage,deb,rpm',
    },
  );
}

async function credentialedBuild(
  fixture: Awaited<ReturnType<typeof linuxBundle>>,
) {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-credentialed-stage-'));
  roots.push(root);
  await cp(fixture.root, join(root, 'bundle'), { recursive: true });
  await cp(fixture.releaseConfigPath, join(root, 'release-config.json'));
  const reviewedBinary = reviewedExecutable('linux', SOURCE_COMMIT);
  await mkdir(join(root, 'reviewed-binary'), { recursive: true });
  await writeFile(join(root, 'reviewed-binary', 'mdreader-app'), reviewedBinary);
  const files: Array<{
    path: string;
    kind: 'file';
    mode: number;
    bytes: number;
    sha256: string;
  }> = [];
  async function walk(directory: string): Promise<void> {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await walk(path);
        continue;
      }
      const body = await readFile(path);
      files.push({
        path: relative(root, path).split(sep).join('/'),
        kind: 'file',
        mode: (await lstat(path)).mode & 0o777,
        bytes: body.byteLength,
        sha256: createHash('sha256').update(body).digest('hex'),
      });
    }
  }
  await walk(root);
  files.sort((left, right) => left.path.localeCompare(right.path));
  const releaseConfigSha256 = files.find(
    (entry) => entry.path === 'release-config.json',
  )!.sha256;
  const manifestPath = join(root, 'credentialed-output.json');
  await writeFile(
    manifestPath,
    `${JSON.stringify(
      {
        schemaVersion: 2,
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        platform: 'linux',
        target: 'x86_64-unknown-linux-gnu',
        bundles: 'appimage,deb,rpm',
        unsignedHandoffSha256: 'a'.repeat(64),
        reviewedBinaryPath: 'reviewed-binary/mdreader-app',
        reviewedBinarySha256: createHash('sha256')
          .update(reviewedBinary)
          .digest('hex'),
        updaterPublicKeySha256: UPDATER_PUBLIC_KEY_SHA256,
        releaseConfigSha256,
        files,
      },
      null,
      2,
    )}\n`,
  );
  const packagePaths = [
    ['appimage', join(root, 'bundle', 'appimage', 'MDReader_1.0.0_amd64.AppImage')],
    ['deb', join(root, 'bundle', 'deb', 'MDReader_1.0.0_amd64.deb')],
    ['rpm', join(root, 'bundle', 'rpm', 'MDReader-1.0.0-1.x86_64.rpm')],
    [
      'updater',
      join(
        root,
        'bundle',
        'appimage',
        'MDReader_1.0.0_amd64.AppImage.tar.gz',
      ),
    ],
  ] as const;
  const evidenceRoot = await mkdtemp(
    join(tmpdir(), 'mdreader-package-identity-'),
  );
  roots.push(evidenceRoot);
  const packageIdentityEvidencePath = join(
    evidenceRoot,
    'package-identity.json',
  );
  await writeFile(
    packageIdentityEvidencePath,
    `${JSON.stringify(
      verifyPackageBinarySet(
        'linux',
        await Promise.all(
          packagePaths.map(async ([kind, path]) => ({
            kind,
            binary: reviewedBinary,
            artifact: await readFile(path),
            signatureAuthenticated: true,
            signatureIdentity: null,
            entrypoint: {
              contract:
                kind === 'appimage' || kind === 'updater'
                  ? 'linux-apprun-target'
                  : 'linux-package-install-target',
              path: 'usr/bin/mdreader-app',
            },
          })),
        ),
        {
          target: 'x86_64-unknown-linux-gnu',
          sourceCommit: SOURCE_COMMIT,
          unsignedHandoffSha256: 'a'.repeat(64),
          reviewedBinarySha256: createHash('sha256')
            .update(reviewedBinary)
            .digest('hex'),
          reviewedBinary,
          expectedPublisherIdentity: null,
          expectedVersion: '1.0.0',
        },
      ),
      null,
      2,
    )}\n`,
  );
  return {
    root,
    manifestPath,
    bundleRoot: join(root, 'bundle'),
    releaseConfigPath: join(root, 'release-config.json'),
    packageIdentityEvidencePath,
  };
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe('release artifact staging', () => {
  test.runIf(process.platform !== 'win32')(
    'the checked-in SBOM inspector runs with the platform system Bash',
    async () => {
      const root = await mkdtemp(join(tmpdir(), 'mdreader-sbom-inspection-'));
      roots.push(root);
      await writeFile(
        join(root, 'MDReader.spdx.json'),
        `${JSON.stringify({ spdxVersion: 'SPDX-2.3' })}\n`,
      );

      await expect(
        execFileAsync(
          '/bin/bash',
          [resolve(repositoryRoot, 'scripts/release/inspect-sboms.sh'), root],
          { cwd: repositoryRoot },
        ),
      ).resolves.toBeDefined();
    },
  );

  test('stages only after reverifying the fresh credentialed-output handoff', async () => {
    const fixture = await linuxBundle();
    const credentialed = await credentialedBuild(fixture);
    const fragment = await stagePlatformArtifacts({
      platform: 'linux',
      version: '1.0.0',
      sourceCommit: SOURCE_COMMIT,
      publishedAt: '2026-07-26T12:00:00.000Z',
      bundleRoot: credentialed.bundleRoot,
      outputRoot: fixture.output,
      releaseConfigPath: credentialed.releaseConfigPath,
      credentialedOutputRoot: credentialed.root,
      credentialedOutputManifestPath: credentialed.manifestPath,
      packageIdentityEvidencePath: credentialed.packageIdentityEvidencePath,
      buildTarget: 'x86_64-unknown-linux-gnu',
      buildBundles: 'appimage,deb,rpm',
      generateSbom: syftFixture,
    });

    expect(fragment.artifacts).toHaveLength(4);
    expect(fragment.releaseConfigSha256).toMatch(/^[0-9a-f]{64}$/);
  });

  test('stages the complete Linux matrix with exact hashes, signatures, and SPDX SBOMs', async () => {
    const fixture = await linuxBundle();
    const { root, output } = fixture;
    const fragment = await stagePlatformArtifacts({
      platform: 'linux',
      version: '1.0.0',
      sourceCommit: SOURCE_COMMIT,
      publishedAt: '2026-07-26T12:00:00.000Z',
      bundleRoot: root,
      outputRoot: output,
      ...attestedBuild(fixture),
      generateSbom: syftFixture,
    });

    expect(fragment.artifacts.map((entry) => entry.kind)).toEqual([
      'appimage',
      'deb',
      'rpm',
      'updater',
    ]);
    expect(fragment.updaterPublicKeySha256).toBe(UPDATER_PUBLIC_KEY_SHA256);
    expect(fragment.releaseConfigSha256).toMatch(/^[0-9a-f]{64}$/);
    expect(
      fragment.artifacts.every(
        (entry) =>
          entry.updaterPublicKeySha256 === fragment.updaterPublicKeySha256 &&
          entry.releaseConfigSha256 === fragment.releaseConfigSha256,
      ),
    ).toBe(true);
    expect(fragment.artifacts.every((entry) => entry.bytes > 0)).toBe(true);
    expect(fragment.artifacts.every((entry) => /^[0-9a-f]{64}$/.test(entry.sha256))).toBe(
      true,
    );
    expect(fragment.artifacts.find((entry) => entry.kind === 'updater')!.signature).toBe(
      SIGNATURE,
    );
    const sbom = JSON.parse(
      await readFile(
        join(output, 'releases', 'v1.0.0', 'MDReader-1.0.0-x86_64.AppImage.spdx.json'),
        'utf8',
      ),
    ) as Record<string, unknown>;
    expect(sbom.spdxVersion).toBe('SPDX-2.3');
    expect(sbom.creationInfo).toMatchObject({
      creators: ['Tool: syft-1.29.0'],
    });
    expect(sbom.packages).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          name: 'MDReader-1.0.0-x86_64.AppImage',
          checksums: [
            expect.objectContaining({
              algorithm: 'SHA256',
              checksumValue: fragment.artifacts[0]!.sha256,
            }),
          ],
        }),
      ]),
    );
    expect(sbom.relationships).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          spdxElementId: 'SPDXRef-DOCUMENT',
          relationshipType: 'DESCRIBES',
        }),
      ]),
    );
  });

  test('refuses a partial Linux matrix instead of emitting a fragment', async () => {
    const fixture = await linuxBundle();
    const { root, output } = fixture;
    await rm(join(root, 'rpm', 'MDReader-1.0.0-1.x86_64.rpm'));
    await reseal(fixture);

    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: root,
        outputRoot: output,
        ...attestedBuild(fixture),
        generateSbom: syftFixture,
      }),
    ).rejects.toThrow(/exactly one .*RPM/i);
  });

  test('refuses a prepopulated staging root that could collide during aggregate merge', async () => {
    const fixture = await linuxBundle();
    await writeFile(
      join(fixture.output, 'fragment-windows.json'),
      '{"unreviewed":"collision"}\n',
    );

    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: fixture.root,
        outputRoot: fixture.output,
        ...attestedBuild(fixture),
        generateSbom: syftFixture,
      }),
    ).rejects.toThrow(/staging root.*empty|collision/i);
  });

  test('refuses artifacts swapped after the attested Tauri build', async () => {
    const fixture = await linuxBundle();
    await writeFile(
      join(fixture.root, 'appimage', 'MDReader_1.0.0_amd64.AppImage'),
      'artifact-built-under-another-updater-key',
    );

    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: fixture.root,
        outputRoot: fixture.output,
        ...attestedBuild(fixture),
        generateSbom: syftFixture,
      }),
    ).rejects.toThrow(/sealed Tauri bundle/);
  });

  test('refuses staging after the embedded updater config changes post-build', async () => {
    const fixture = await linuxBundle();
    await writeFile(
      fixture.releaseConfigPath,
      `${JSON.stringify(
        createReleaseConfig({
          version: '1.0.0',
          workspaceVersion: '1.0.0',
          bundleIdentifier: 'dev.mdreader.app',
          updaterPublicKey: 'W'.repeat(88),
          updaterEndpoint:
            'https://downloads.mdreader.dev/channels/stable/updater.json',
        }),
        null,
        2,
      )}\n`,
    );

    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: fixture.root,
        outputRoot: fixture.output,
        ...attestedBuild(fixture),
        generateSbom: syftFixture,
      }),
    ).rejects.toThrow(/exact config consumed by the Tauri build/);
  });

  test('refuses a marker-only document that was not produced by the pinned scanner', async () => {
    const fixture = await linuxBundle();
    const { root, output } = fixture;
    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: root,
        outputRoot: output,
        ...attestedBuild(fixture),
        generateSbom: async () => '{"spdxVersion":"SPDX-2.3"}',
      }),
    ).rejects.toThrow(/pinned Syft|complete SPDX/);
  });

  test('refuses genuine Syft output that contains only unrelated packages', async () => {
    const fixture = await linuxBundle();
    const { root, output } = fixture;
    await expect(
      stagePlatformArtifacts({
        platform: 'linux',
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        publishedAt: '2026-07-26T12:00:00.000Z',
        bundleRoot: root,
        outputRoot: output,
        ...attestedBuild(fixture),
        generateSbom: async () =>
          JSON.stringify({
            spdxVersion: 'SPDX-2.3',
            dataLicense: 'CC0-1.0',
            SPDXID: 'SPDXRef-DOCUMENT',
            creationInfo: { creators: ['Tool: syft-1.29.0'] },
            packages: [
              {
                SPDXID: 'SPDXRef-Package-runtime',
                name: 'runtime-dependency',
              },
            ],
            relationships: [
              {
                spdxElementId: 'SPDXRef-DOCUMENT',
                relationshipType: 'DESCRIBES',
                relatedSpdxElement: 'SPDXRef-Package-runtime',
              },
            ],
          }),
      }),
    ).rejects.toThrow(/complete SPDX|exact release artifact checksum/);
  });
});
