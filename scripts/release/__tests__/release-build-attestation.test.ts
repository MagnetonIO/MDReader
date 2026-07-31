import { createHash } from 'node:crypto';
import {
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, describe, expect, test } from 'vitest';

import {
  attestReleaseBuildConfig,
  sealReleaseBundle,
  verifyCredentialedOutput,
  verifyReleaseBundleSeal,
  verifyReleaseBuildAttestation,
} from '../release-build-attestation';
import {
  createReleaseConfig,
  updaterPublicKeyFingerprint,
} from '../release-config';
import { reviewedExecutable } from './release-binary-fixtures';

const roots: string[] = [];
const keyA = 'A'.repeat(88);
const keyB = 'B'.repeat(88);
const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const expected = {
  version: '1.0.0',
  target: 'x86_64-unknown-linux-gnu',
  bundles: 'appimage,deb,rpm',
};

function config(key: string) {
  return createReleaseConfig({
    version: expected.version,
    workspaceVersion: expected.version,
    bundleIdentifier: 'dev.mdreader.app',
    updaterPublicKey: key,
    updaterEndpoint:
      'https://downloads.mdreader.dev/channels/stable/updater.json',
  });
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('direct Tauri build attestation', () => {
  test('binds staging to the exact config bytes and updater key consumed by the build', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-build-attestation-'));
    roots.push(root);
    const configPath = join(root, 'release-config.json');
    const attestationPath = join(root, 'build-attestation.json');
    await writeFile(configPath, `${JSON.stringify(config(keyA), null, 2)}\n`);

    const attestation = await attestReleaseBuildConfig(
      configPath,
      attestationPath,
      expected.target,
      expected.bundles,
    );
    expect(attestation.updaterPublicKeySha256).toBe(
      updaterPublicKeyFingerprint(keyA),
    );
    await expect(
      verifyReleaseBuildAttestation(configPath, attestationPath, expected),
    ).resolves.toEqual(attestation);

    await writeFile(configPath, `${JSON.stringify(config(keyB), null, 2)}\n`);
    await expect(
      verifyReleaseBuildAttestation(configPath, attestationPath, expected),
    ).rejects.toThrow(/exact config consumed by the Tauri build/);
  });

  test('rejects any artifact replacement after the attested Tauri build', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-bundle-seal-'));
    roots.push(root);
    const configPath = join(root, 'release-config.json');
    const attestationPath = join(root, 'build-attestation.json');
    const bundleRoot = join(root, 'bundle');
    const sealPath = join(root, 'bundle-seal.json');
    await mkdir(bundleRoot);
    await writeFile(configPath, `${JSON.stringify(config(keyA), null, 2)}\n`);
    await writeFile(join(bundleRoot, 'MDReader.AppImage'), 'built-under-key-A');
    await attestReleaseBuildConfig(
      configPath,
      attestationPath,
      expected.target,
      expected.bundles,
    );

    await sealReleaseBundle(
      bundleRoot,
      configPath,
      attestationPath,
      sealPath,
      expected,
    );
    await expect(
      verifyReleaseBundleSeal(
        bundleRoot,
        configPath,
        attestationPath,
        sealPath,
        expected,
      ),
    ).resolves.toBeUndefined();

    await writeFile(join(bundleRoot, 'MDReader.AppImage'), 'built-under-key-B');
    await expect(
      verifyReleaseBundleSeal(
        bundleRoot,
        configPath,
        attestationPath,
        sealPath,
        expected,
      ),
    ).rejects.toThrow(/sealed Tauri bundle/);
  });

  test('rejects Windows signer replacement after its bytes were attested', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-signer-attestation-'));
    roots.push(root);
    const releaseScripts = join(root, 'scripts', 'release');
    const signerPath = join(
      releaseScripts,
      'sign-windows-from-store.ps1',
    );
    const configPath = join(root, 'release-config.json');
    const attestationPath = join(root, 'build-attestation.json');
    const windowsExpected = {
      version: expected.version,
      target: 'x86_64-pc-windows-msvc',
      bundles: 'nsis',
    };
    await mkdir(releaseScripts, { recursive: true });
    await writeFile(signerPath, 'Write-Output "reviewed signer"\n');
    await writeFile(
      configPath,
      `${JSON.stringify(
        createReleaseConfig({
          version: windowsExpected.version,
          workspaceVersion: windowsExpected.version,
          bundleIdentifier: 'dev.mdreader.app',
          updaterPublicKey: keyA,
          updaterEndpoint:
            'https://downloads.mdreader.dev/channels/stable/updater.json',
          windowsCertificateThumbprint:
            '0123456789ABCDEF0123456789ABCDEF01234567',
          windowsInstallerHooks: join(
            releaseScripts,
            'windows-minimum.nsh',
          ),
          windowsSignCommand: signerPath,
        }),
        null,
        2,
      )}\n`,
    );

    await attestReleaseBuildConfig(
      configPath,
      attestationPath,
      windowsExpected.target,
      windowsExpected.bundles,
    );
    await writeFile(signerPath, 'Write-Output "replacement signer"\n');

    await expect(
      verifyReleaseBuildAttestation(
        configPath,
        attestationPath,
        windowsExpected,
      ),
    ).rejects.toThrow(/Windows signer|signer bytes/i);
  });
});

describe('closed credentialed output', () => {
  async function fixture() {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-credentialed-output-'));
    roots.push(root);
    const configPath = join(root, 'release-config.json');
    const artifactPath = join(
      root,
      'bundle',
      'appimage',
      'MDReader.AppImage',
    );
    const reviewedBinaryPath = join(root, 'reviewed-binary', 'mdreader-app');
    await mkdir(join(root, 'bundle', 'appimage'), { recursive: true });
    await mkdir(join(root, 'reviewed-binary'), { recursive: true });
    await writeFile(configPath, `${JSON.stringify(config(keyA), null, 2)}\n`);
    await writeFile(artifactPath, 'signed-release-artifact');
    await writeFile(
      reviewedBinaryPath,
      reviewedExecutable('linux', SOURCE_COMMIT),
    );
    const entries = (
      await Promise.all(
      [configPath, artifactPath, reviewedBinaryPath].map(async (path) => {
        const body = await readFile(path);
        return {
          path:
            path === configPath
              ? 'release-config.json'
              : path === artifactPath
                ? 'bundle/appimage/MDReader.AppImage'
                : 'reviewed-binary/mdreader-app',
          kind: 'file' as const,
          mode: (await lstat(path)).mode & 0o777,
          bytes: body.byteLength,
          sha256: createHash('sha256').update(body).digest('hex'),
        };
      }),
      )
    ).sort((left, right) => left.path.localeCompare(right.path));
    const manifestPath = join(root, 'credentialed-output.json');
    await writeFile(
      manifestPath,
      `${JSON.stringify(
        {
          schemaVersion: 2,
          version: expected.version,
          sourceCommit: SOURCE_COMMIT,
          platform: 'linux',
          target: expected.target,
          bundles: expected.bundles,
          unsignedHandoffSha256: 'a'.repeat(64),
          reviewedBinaryPath: 'reviewed-binary/mdreader-app',
          reviewedBinarySha256: entries.find(
            (entry) => entry.path === 'reviewed-binary/mdreader-app',
          )!.sha256,
          updaterPublicKeySha256: updaterPublicKeyFingerprint(keyA),
          releaseConfigSha256: entries.find(
            (entry) => entry.path === 'release-config.json',
          )!.sha256,
          files: entries,
        },
        null,
        2,
      )}\n`,
    );
    return { root, configPath, artifactPath, manifestPath };
  }

  const credentialedExpected = {
    version: expected.version,
    sourceCommit: SOURCE_COMMIT,
    platform: 'linux' as const,
    target: expected.target,
    bundles: expected.bundles,
  };

  test('binds the exact protected-job config and bundle inventory to the platform tuple', async () => {
    const input = await fixture();

    await expect(
      verifyCredentialedOutput(
        input.root,
        input.manifestPath,
        credentialedExpected,
      ),
    ).resolves.toMatchObject({
      releaseConfigSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
      updaterPublicKeySha256: updaterPublicKeyFingerprint(keyA),
    });
  });

  test('rejects bundle replacement after the credentialed output was sealed', async () => {
    const input = await fixture();
    await writeFile(input.artifactPath, 'replacement');

    await expect(
      verifyCredentialedOutput(
        input.root,
        input.manifestPath,
        credentialedExpected,
      ),
    ).rejects.toThrow(/credentialed output.*inventory/i);
  });

  test('rejects a platform target or bundle tuple mixed after sealing', async () => {
    const input = await fixture();

    await expect(
      verifyCredentialedOutput(input.root, input.manifestPath, {
        ...credentialedExpected,
        platform: 'windows',
      }),
    ).rejects.toThrow(/credentialed output.*platform tuple/i);
  });
});
