import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';

import { updaterPublicKeyFingerprint } from './release-config';

const SHA256 = /^[0-9a-f]{64}$/;
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const PUBLIC_KEY = /^[A-Za-z0-9+/]+={0,2}$/;

export interface ReleaseBuildAttestation {
  schemaVersion: 1;
  version: string;
  target: string;
  bundles: string;
  releaseConfigSha256: string;
  updaterPublicKeySha256: string;
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function releaseConfigKey(input: unknown): {
  version: string;
  updaterPublicKey: string;
} {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) {
    throw new Error('release build config must be a JSON object');
  }
  const config = input as {
    identifier?: unknown;
    version?: unknown;
    bundle?: { createUpdaterArtifacts?: unknown };
    plugins?: { updater?: { endpoints?: unknown; pubkey?: unknown } };
  };
  if (
    config.identifier !== 'dev.mdreader.app' ||
    typeof config.version !== 'string' ||
    !SEMVER.test(config.version) ||
    config.bundle?.createUpdaterArtifacts !== true ||
    !Array.isArray(config.plugins?.updater?.endpoints) ||
    config.plugins.updater.endpoints.length !== 1 ||
    config.plugins.updater.endpoints[0] !==
      'https://downloads.mdreader.dev/channels/stable/updater.json' ||
    typeof config.plugins.updater.pubkey !== 'string' ||
    config.plugins.updater.pubkey.length < 64 ||
    config.plugins.updater.pubkey.length % 4 !== 0 ||
    !PUBLIC_KEY.test(config.plugins.updater.pubkey)
  ) {
    throw new Error(
      'release build config does not contain the reviewed direct updater identity',
    );
  }
  return {
    version: config.version,
    updaterPublicKey: config.plugins.updater.pubkey,
  };
}

export async function attestReleaseBuildConfig(
  configPath: string,
  outputPath: string,
  target: string,
  bundles: string,
): Promise<ReleaseBuildAttestation> {
  const body = await readFile(configPath);
  const parsed = releaseConfigKey(JSON.parse(body.toString('utf8')));
  const attestation: ReleaseBuildAttestation = {
    schemaVersion: 1,
    version: parsed.version,
    target,
    bundles,
    releaseConfigSha256: sha256(body),
    updaterPublicKeySha256: updaterPublicKeyFingerprint(
      parsed.updaterPublicKey,
    ),
  };
  await writeFile(outputPath, `${JSON.stringify(attestation, null, 2)}\n`, {
    mode: 0o600,
  });
  return attestation;
}

export async function verifyReleaseBuildAttestation(
  configPath: string,
  attestationPath: string,
  expected: { version: string; target: string; bundles: string },
): Promise<ReleaseBuildAttestation> {
  const [body, encodedAttestation] = await Promise.all([
    readFile(configPath),
    readFile(attestationPath, 'utf8'),
  ]);
  let attestation: ReleaseBuildAttestation;
  try {
    attestation = JSON.parse(encodedAttestation) as ReleaseBuildAttestation;
  } catch {
    throw new Error('release build attestation is not valid JSON');
  }
  const parsed = releaseConfigKey(JSON.parse(body.toString('utf8')));
  if (
    attestation.schemaVersion !== 1 ||
    attestation.version !== expected.version ||
    attestation.target !== expected.target ||
    attestation.bundles !== expected.bundles ||
    !SHA256.test(attestation.releaseConfigSha256) ||
    !SHA256.test(attestation.updaterPublicKeySha256) ||
    attestation.releaseConfigSha256 !== sha256(body) ||
    attestation.updaterPublicKeySha256 !==
      updaterPublicKeyFingerprint(parsed.updaterPublicKey) ||
    parsed.version !== expected.version
  ) {
    throw new Error(
      'release build attestation does not match the exact config consumed by the Tauri build',
    );
  }
  return attestation;
}
