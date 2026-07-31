import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';

import { describe, expect, test } from 'vitest';

import {
  createReleaseConfig,
  readWorkspaceVersion,
  updaterPublicKeyFingerprint,
} from '../release-config';

const root = resolve(process.cwd(), '..', '..');
const updaterKey = 'V'.repeat(88);
const generator = 'scripts/release/generate-direct-release-config.sh';
// Start from an uncredentialed environment unless a test deliberately injects
// an ambient signing value to prove the unsigned generator ignores it.
const AMBIENT_SIGNING_VARIABLES = [
  'APPLE_SIGNING_IDENTITY',
  'APPLE_CERTIFICATE',
  'APPLE_CERTIFICATE_PASSWORD',
  'APPLE_TEAM_ID',
  'WINDOWS_CERT_THUMBPRINT',
  'MDREADER_MACOS_ORT_DYLIB',
];

function generatorEnvironment(
  overrides: Record<string, string> = {},
): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = {
    ...process.env,
    MDREADER_UPDATER_PUBLIC_KEY: updaterKey,
  };
  for (const name of AMBIENT_SIGNING_VARIABLES) delete environment[name];
  return { ...environment, ...overrides };
}

function generate(
  platform: string,
  overrides: Record<string, string> = {},
): { bundle: Record<string, unknown> } {
  const directory = mkdtempSync(resolve(tmpdir(), 'mdreader-release-config-'));
  const output = resolve(directory, 'release-config.json');
  try {
    execFileSync(
      '/bin/bash',
      [joinRoot(generator), platform, '1.0.0', output],
      {
        cwd: root,
        env: generatorEnvironment(overrides),
        stdio: 'pipe',
      },
    );
    return JSON.parse(readFileSync(output, 'utf8')) as {
      bundle: Record<string, unknown>;
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe('release version and updater configuration', () => {
  test('fingerprints the exact public-key string embedded in release builds', () => {
    expect(updaterPublicKeyFingerprint('embedded-key-A')).toBe(
      'aa7eaf4dd1dae232809791d55c6f4b44e9a78ccd85b52ac7d1c50d79a5f095f5',
    );
    expect(updaterPublicKeyFingerprint('embedded-key-B')).not.toBe(
      updaterPublicKeyFingerprint('embedded-key-A'),
    );
  });

  test('Cargo is the single non-placeholder application version source', () => {
    expect(readWorkspaceVersion(joinRoot('Cargo.toml'))).toBe('1.0.0');
    const tauri = JSON.parse(
      readFileSync(joinRoot('apps/mdreader/src-tauri/tauri.conf.json'), 'utf8'),
    ) as Record<string, unknown>;
    expect(tauri).not.toHaveProperty('version');
  });

  test('generates a direct-channel updater overlay only for the owning version', () => {
    const config = createReleaseConfig({
      version: '1.0.0',
      workspaceVersion: '1.0.0',
      bundleIdentifier: 'dev.mdreader.app',
      updaterPublicKey: updaterKey,
      updaterEndpoint: 'https://downloads.mdreader.dev/channels/stable/updater.json',
      macosSigningIdentity: 'Developer ID Application: Example Corp (ABCDE12345)',
      macosFramework:
        '/reviewed/runtime/libonnxruntime.1.24.2.dylib',
      windowsCertificateThumbprint: '0123456789ABCDEF0123456789ABCDEF01234567',
      windowsInstallerHooks: '/repo/scripts/release/windows-minimum.nsh',
      windowsSignCommand:
        '/repo/scripts/release/sign-windows-from-store.ps1',
    });

    expect(config).toMatchObject({
      identifier: 'dev.mdreader.app',
      version: '1.0.0',
      build: {
        beforeBuildCommand: '',
      },
      bundle: {
        createUpdaterArtifacts: true,
        macOS: {
          minimumSystemVersion: '12.0',
          signingIdentity: 'Developer ID Application: Example Corp (ABCDE12345)',
          frameworks: [
            '/reviewed/runtime/libonnxruntime.1.24.2.dylib',
          ],
        },
        windows: {
          signCommand: {
            cmd: 'pwsh',
            args: [
              '-NoProfile',
              '-NonInteractive',
              '-File',
              '/repo/scripts/release/sign-windows-from-store.ps1',
              '%1',
            ],
          },
          nsis: {
            installerHooks: '/repo/scripts/release/windows-minimum.nsh',
          },
        },
      },
      plugins: {
        updater: {
          endpoints: ['https://downloads.mdreader.dev/channels/stable/updater.json'],
          pubkey: updaterKey,
        },
      },
    });
    expect(config.bundle.windows).not.toHaveProperty('certificateThumbprint');
  });

  test.runIf(process.platform !== 'win32')(
    'generates the credential-free macOS release overlay with the system shell',
    () => {
      const generated = generate('macos', {
        MDREADER_MACOS_ORT_DYLIB:
          '/reviewed/runtime/libonnxruntime.1.24.2.dylib',
      });
      expect(generated.bundle.createUpdaterArtifacts).toBe(true);
      expect(generated.bundle).toMatchObject({
        macOS: {
          minimumSystemVersion: '12.0',
          frameworks: [
            '/reviewed/runtime/libonnxruntime.1.24.2.dylib',
          ],
        },
      });
      expect(
        (generated.bundle.macOS as Record<string, unknown>),
      ).not.toHaveProperty('signingIdentity');
      expect(generated.bundle).not.toHaveProperty('windows');
    },
  );

  test.runIf(process.platform !== 'win32')(
    'generates the uncredentialed Windows overlay without a certificate thumbprint',
    () => {
      const generated = generate('windows');
      expect(generated.bundle.createUpdaterArtifacts).toBe(true);
      expect(generated.bundle).not.toHaveProperty('windows');
      expect(generated.bundle).not.toHaveProperty('macOS');
    },
  );

  test.runIf(process.platform !== 'win32')(
    'ignores ambient signing identities in unsigned macOS and Windows config',
    () => {
      const macos = generate('macos', {
        MDREADER_MACOS_ORT_DYLIB:
          '/reviewed/runtime/libonnxruntime.1.24.2.dylib',
        APPLE_SIGNING_IDENTITY:
          'Developer ID Application: Ambient Shell (ABCDE12345)',
      });
      expect(macos.bundle.macOS).not.toHaveProperty('signingIdentity');

      const windows = generate('windows', {
        WINDOWS_CERT_THUMBPRINT:
          '0123456789ABCDEF0123456789ABCDEF01234567',
      });
      expect(windows.bundle).not.toHaveProperty('windows');
    },
  );

  test.runIf(process.platform !== 'win32')(
    'generates the Linux overlay with neither macOS nor Windows signing inputs',
    () => {
      const generated = generate('linux');
      expect(generated.bundle.createUpdaterArtifacts).toBe(true);
      expect(generated.bundle).not.toHaveProperty('windows');
      expect(generated.bundle).not.toHaveProperty('macOS');
    },
  );

  test.runIf(process.platform !== 'win32')(
    'refuses an unreviewed release platform and a macOS run without its runtime',
    () => {
      expect(() => generate('freebsd')).toThrow();
      expect(() => generate('macos')).toThrow();
    },
  );

  test('rejects a divergent version, missing key, or unsafe updater endpoint', () => {
    expect(() =>
      createReleaseConfig({
        version: '1.0.1',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: updaterKey,
        updaterEndpoint: 'https://downloads.mdreader.dev/channels/stable/updater.json',
      }),
    ).toThrow(/workspace version/);
    expect(() =>
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: '',
        updaterEndpoint: 'https://downloads.mdreader.dev/channels/stable/updater.json',
      }),
    ).toThrow(/public key/);
    expect(() =>
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: updaterKey,
        updaterEndpoint: 'http://downloads.mdreader.dev/latest.json',
      }),
    ).toThrow(/HTTPS/);
    expect(() =>
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: updaterKey,
        updaterEndpoint: 'https://evil.example/channels/stable/updater.json',
      }),
    ).toThrow(/canonical updater endpoint/);
  });

  test.each([
    '1.0.0-alpha',
    '1.0.0-A.2',
    '1.0.0+build.7',
    '01.0.0',
    '1.00.0',
    '1.0.00',
  ])('rejects non-stable release version %s', (version) => {
    expect(() =>
      createReleaseConfig({
        version,
        workspaceVersion: version,
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: updaterKey,
        updaterEndpoint:
          'https://downloads.mdreader.dev/channels/stable/updater.json',
      }),
    ).toThrow(/stable semantic version/);
  });

  test.each([
    'https://downloads.mdreader.dev:443/channels/stable/updater.json',
    'https://DOWNLOADS.mdreader.dev/channels/stable/updater.json',
    'https://downloads.mdreader.dev/channels/stable/extra/../updater.json',
    'https://downloads.mdreader.dev/channels/stable/%75pdater.json',
    'https://downloads.mdreader.dev/channels/stable/updater․json',
  ])('rejects normalized but noncanonical raw updater endpoint %s', (updaterEndpoint) => {
    expect(() =>
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: updaterKey,
        updaterEndpoint,
      }),
    ).toThrow(/canonical updater endpoint/);
  });

  test('refuses to compose the direct updater with the E2E identity', () => {
    expect(() =>
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app.e2e',
        updaterPublicKey: updaterKey,
        updaterEndpoint: 'https://downloads.mdreader.dev/channels/stable/updater.json',
      }),
    ).toThrow(/production bundle identifier/);
  });

  test('E2E remains a distinct updater-free build contract', () => {
    const e2e = JSON.parse(
      readFileSync(joinRoot('apps/mdreader/src-tauri/tauri.e2e.conf.json'), 'utf8'),
    ) as {
      identifier: string;
      mainBinaryName: string;
      bundle: { createUpdaterArtifacts: boolean };
      plugins?: unknown;
    };
    expect(e2e.identifier).toBe('dev.mdreader.app.e2e');
    expect(e2e.mainBinaryName).toBe('mdreader-e2e');
    expect(e2e.bundle.createUpdaterArtifacts).toBe(false);
    expect(e2e.plugins).toBeUndefined();
    const harness = JSON.parse(
      readFileSync(joinRoot('apps/mdreader/e2e-native/package.json'), 'utf8'),
    ) as { scripts: { 'build:e2e': string } };
    expect(harness.scripts['build:e2e']).toBe('node build-e2e.mjs');
    const e2eBuilder = readFileSync(
      joinRoot('apps/mdreader/e2e-native/build-e2e.mjs'),
      'utf8',
    );
    expect(e2eBuilder).toMatch(/['"]--features['"]\s*,\s*['"]e2e-build['"]/);
    const cargo = readFileSync(joinRoot('apps/mdreader/src-tauri/Cargo.toml'), 'utf8');
    const library = readFileSync(joinRoot('apps/mdreader/src-tauri/src/lib.rs'), 'utf8');
    expect(cargo).toMatch(/e2e-build\s*=\s*\[\]/);
    expect(library).toMatch(
      /cfg\(all\(feature = "direct-updater", feature = "e2e-build"\)\)/,
    );
    const tauriWorkflow = readFileSync(
      joinRoot('.github/workflows/tauri.yml'),
      'utf8',
    );
    const e2eBuild = tauriWorkflow
      .split('\n')
      .find((line) => line.includes('tauri build') && line.includes('tauri.e2e.conf.json'));
    expect(e2eBuild).toContain('--features e2e-build');

    const isolation = readFileSync(
      joinRoot('scripts/release/check-e2e-updater-isolation.sh'),
      'utf8',
    );
    expect(isolation).toContain(
      'cargo check -p mdreader-app --no-default-features --features e2e-build,direct-updater',
    );
    expect(isolation).toContain(
      'the direct updater cannot be compiled into the isolated E2E application',
    );
  });

  test('the Windows installer enforces the minimum represented in the manifest', () => {
    const hook = readFileSync(
      joinRoot('scripts/release/windows-minimum.nsh'),
      'utf8',
    );
    expect(hook).toContain('${AtLeastWin10}');
    expect(hook).toMatch(/\bAbort\b/);
  });
});

function joinRoot(path: string): string {
  return resolve(root, path);
}
