import { readFileSync } from 'node:fs';

const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const PUBLIC_KEY = /^[A-Za-z0-9+/]+={0,2}$/;
const WINDOWS_THUMBPRINT = /^[0-9A-F]{40}$/;

export interface ReleaseConfigInput {
  version: string;
  workspaceVersion: string;
  bundleIdentifier: string;
  updaterPublicKey: string;
  updaterEndpoint: string;
  macosSigningIdentity?: string;
  windowsCertificateThumbprint?: string;
  windowsInstallerHooks?: string;
}

export interface DirectReleaseConfig {
  identifier: 'dev.mdreader.app';
  version: string;
  bundle: {
    createUpdaterArtifacts: true;
    macOS?: {
      minimumSystemVersion: '12.0';
      signingIdentity: string;
    };
    windows?: {
      certificateThumbprint: string;
      digestAlgorithm: 'sha256';
      nsis: {
        installerHooks: string;
      };
      timestampUrl: string;
    };
  };
  plugins: {
    updater: {
      endpoints: string[];
      pubkey: string;
    };
  };
}

export function readWorkspaceVersion(cargoTomlPath: string): string {
  const source = readFileSync(cargoTomlPath, 'utf8');
  const marker = '[workspace.package]';
  const start = source.indexOf(marker);
  const remainder = start < 0 ? '' : source.slice(start + marker.length);
  const nextSection = remainder.search(/^\s*\[/m);
  const section = nextSection < 0 ? remainder : remainder.slice(0, nextSection);
  const version = section?.match(/^\s*version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  if (!version || !SEMVER.test(version)) {
    throw new Error('Cargo workspace.package.version must be a semantic version');
  }
  if (version === '0.0.0') throw new Error('Cargo workspace version cannot be 0.0.0');
  return version;
}

export function readTauriIdentifier(tauriConfigPath: string): string {
  const config = JSON.parse(readFileSync(tauriConfigPath, 'utf8')) as {
    identifier?: unknown;
  };
  if (typeof config.identifier !== 'string' || config.identifier.length === 0) {
    throw new Error('Tauri bundle identifier is missing');
  }
  return config.identifier;
}

export function createReleaseConfig(input: ReleaseConfigInput): DirectReleaseConfig {
  if (!SEMVER.test(input.version)) throw new Error('release version must be semantic');
  if (input.version !== input.workspaceVersion) {
    throw new Error(
      `release version ${input.version} must equal workspace version ${input.workspaceVersion}`,
    );
  }
  if (input.bundleIdentifier !== 'dev.mdreader.app') {
    throw new Error('direct updater requires the production bundle identifier dev.mdreader.app');
  }
  if (
    input.updaterPublicKey.length < 64 ||
    input.updaterPublicKey.length % 4 !== 0 ||
    !PUBLIC_KEY.test(input.updaterPublicKey)
  ) {
    throw new Error('updater public key must be a non-empty base64 key');
  }
  let endpoint: URL;
  try {
    endpoint = new URL(input.updaterEndpoint);
  } catch {
    throw new Error('updater endpoint must be a valid HTTPS URL');
  }
  if (
    endpoint.protocol !== 'https:' ||
    endpoint.username !== '' ||
    endpoint.password !== '' ||
    endpoint.search !== '' ||
    endpoint.hash !== ''
  ) {
    throw new Error('updater endpoint must be an HTTPS URL without credentials, query, or fragment');
  }
  if (
    endpoint.origin !== 'https://downloads.mdreader.dev' ||
    endpoint.pathname !== '/channels/stable/updater.json'
  ) {
    throw new Error(
      'direct releases require the canonical updater endpoint on downloads.mdreader.dev',
    );
  }
  if (
    input.macosSigningIdentity !== undefined &&
    input.macosSigningIdentity.trim().length === 0
  ) {
    throw new Error('macOS signing identity cannot be empty');
  }
  if (
    input.windowsCertificateThumbprint !== undefined &&
    !WINDOWS_THUMBPRINT.test(input.windowsCertificateThumbprint)
  ) {
    throw new Error('Windows certificate thumbprint must be an uppercase SHA-1 fingerprint');
  }
  if (
    input.windowsCertificateThumbprint !== undefined &&
    (!input.windowsInstallerHooks ||
      !input.windowsInstallerHooks.endsWith('scripts/release/windows-minimum.nsh'))
  ) {
    throw new Error('Windows direct releases require the reviewed minimum-OS installer hook');
  }

  return {
    identifier: 'dev.mdreader.app',
    version: input.version,
    bundle: {
      createUpdaterArtifacts: true,
      ...(input.macosSigningIdentity
        ? {
            macOS: {
              minimumSystemVersion: '12.0',
              signingIdentity: input.macosSigningIdentity,
            },
          }
        : {}),
      ...(input.windowsCertificateThumbprint
        ? {
            windows: {
              certificateThumbprint: input.windowsCertificateThumbprint,
              digestAlgorithm: 'sha256' as const,
              nsis: {
                installerHooks: input.windowsInstallerHooks!,
              },
              timestampUrl: 'http://timestamp.digicert.com',
            },
          }
        : {}),
    },
    plugins: {
      updater: {
        endpoints: [endpoint.toString()],
        pubkey: input.updaterPublicKey,
      },
    },
  };
}
