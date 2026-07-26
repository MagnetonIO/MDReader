import { createHash } from 'node:crypto';
import {
  readFile,
  readlink,
  readdir,
  stat,
  writeFile,
} from 'node:fs/promises';
import { isAbsolute, join, relative, sep } from 'node:path';

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
  windowsSignerSha256: string | null;
}

interface ReleaseBundleEntry {
  path: string;
  kind: 'file' | 'symlink';
  bytes: number;
  sha256: string;
}

interface ReleaseBundleSeal {
  schemaVersion: 1;
  version: string;
  target: string;
  bundles: string;
  releaseConfigSha256: string;
  updaterPublicKeySha256: string;
  windowsSignerSha256: string | null;
  files: ReleaseBundleEntry[];
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function releaseConfigKey(input: unknown): {
  version: string;
  updaterPublicKey: string;
  windowsSignerPath?: string;
} {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) {
    throw new Error('release build config must be a JSON object');
  }
  const config = input as {
    identifier?: unknown;
    version?: unknown;
    bundle?: {
      createUpdaterArtifacts?: unknown;
      windows?: {
        signCommand?: {
          cmd?: unknown;
          args?: unknown;
        };
      };
    };
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
  const signCommand = config.bundle?.windows?.signCommand;
  let windowsSignerPath: string | undefined;
  if (signCommand !== undefined) {
    if (
      signCommand.cmd !== 'pwsh' ||
      !Array.isArray(signCommand.args) ||
      signCommand.args.length !== 5 ||
      signCommand.args[0] !== '-NoProfile' ||
      signCommand.args[1] !== '-NonInteractive' ||
      signCommand.args[2] !== '-File' ||
      typeof signCommand.args[3] !== 'string' ||
      !isAbsolute(signCommand.args[3]) ||
      !/[\\/]scripts[\\/]release[\\/]sign-windows-from-store\.ps1$/.test(
        signCommand.args[3],
      ) ||
      signCommand.args[4] !== '%1'
    ) {
      throw new Error(
        'release build config does not contain the reviewed Windows signer path',
      );
    }
    windowsSignerPath = signCommand.args[3];
  }
  return {
    version: config.version,
    updaterPublicKey: config.plugins.updater.pubkey,
    windowsSignerPath,
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
  const windowsSignerSha256 = parsed.windowsSignerPath
    ? sha256(await readFile(parsed.windowsSignerPath))
    : null;
  const attestation: ReleaseBuildAttestation = {
    schemaVersion: 1,
    version: parsed.version,
    target,
    bundles,
    releaseConfigSha256: sha256(body),
    updaterPublicKeySha256: updaterPublicKeyFingerprint(
      parsed.updaterPublicKey,
    ),
    windowsSignerSha256,
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
  const windowsSignerSha256 = parsed.windowsSignerPath
    ? sha256(await readFile(parsed.windowsSignerPath))
    : null;
  if (
    attestation.schemaVersion !== 1 ||
    attestation.version !== expected.version ||
    attestation.target !== expected.target ||
    attestation.bundles !== expected.bundles ||
    !SHA256.test(attestation.releaseConfigSha256) ||
    !SHA256.test(attestation.updaterPublicKeySha256) ||
    (attestation.windowsSignerSha256 !== null &&
      !SHA256.test(attestation.windowsSignerSha256)) ||
    attestation.releaseConfigSha256 !== sha256(body) ||
    attestation.updaterPublicKeySha256 !==
      updaterPublicKeyFingerprint(parsed.updaterPublicKey) ||
    attestation.windowsSignerSha256 !== windowsSignerSha256 ||
    parsed.version !== expected.version
  ) {
    throw new Error(
      'release build attestation does not match the exact config consumed by the Tauri build or the exact Windows signer bytes',
    );
  }
  return attestation;
}

async function bundleInventory(
  root: string,
  directory = root,
): Promise<ReleaseBundleEntry[]> {
  const output: ReleaseBundleEntry[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    const relativePath = relative(root, path).split(sep).join('/');
    if (entry.isDirectory()) {
      output.push(...(await bundleInventory(root, path)));
    } else if (entry.isFile()) {
      const body = await readFile(path);
      output.push({
        path: relativePath,
        kind: 'file',
        bytes: (await stat(path)).size,
        sha256: sha256(body),
      });
    } else if (entry.isSymbolicLink()) {
      const target = Buffer.from(await readlink(path), 'utf8');
      output.push({
        path: relativePath,
        kind: 'symlink',
        bytes: target.byteLength,
        sha256: sha256(target),
      });
    } else {
      throw new Error(`unsupported release bundle entry ${relativePath}`);
    }
  }
  return output.sort((left, right) => left.path.localeCompare(right.path));
}

export async function sealReleaseBundle(
  bundleRoot: string,
  configPath: string,
  attestationPath: string,
  outputPath: string,
  expected: { version: string; target: string; bundles: string },
): Promise<void> {
  const attestation = await verifyReleaseBuildAttestation(
    configPath,
    attestationPath,
    expected,
  );
  const files = await bundleInventory(bundleRoot);
  if (files.length === 0) {
    throw new Error('attested Tauri build produced an empty bundle directory');
  }
  const seal: ReleaseBundleSeal = {
    schemaVersion: 1,
    version: expected.version,
    target: expected.target,
    bundles: expected.bundles,
    releaseConfigSha256: attestation.releaseConfigSha256,
    updaterPublicKeySha256: attestation.updaterPublicKeySha256,
    windowsSignerSha256: attestation.windowsSignerSha256,
    files,
  };
  await writeFile(outputPath, `${JSON.stringify(seal, null, 2)}\n`, {
    mode: 0o600,
  });
}

export async function verifyReleaseBundleSeal(
  bundleRoot: string,
  configPath: string,
  attestationPath: string,
  sealPath: string,
  expected: { version: string; target: string; bundles: string },
): Promise<void> {
  const [attestation, encodedSeal, files] = await Promise.all([
    verifyReleaseBuildAttestation(configPath, attestationPath, expected),
    readFile(sealPath, 'utf8'),
    bundleInventory(bundleRoot),
  ]);
  let seal: ReleaseBundleSeal;
  try {
    seal = JSON.parse(encodedSeal) as ReleaseBundleSeal;
  } catch {
    throw new Error('release bundle seal is not valid JSON');
  }
  if (
    seal.schemaVersion !== 1 ||
    seal.version !== expected.version ||
    seal.target !== expected.target ||
    seal.bundles !== expected.bundles ||
    seal.releaseConfigSha256 !== attestation.releaseConfigSha256 ||
    seal.updaterPublicKeySha256 !== attestation.updaterPublicKeySha256 ||
    seal.windowsSignerSha256 !== attestation.windowsSignerSha256 ||
    JSON.stringify(seal.files) !== JSON.stringify(files)
  ) {
    throw new Error(
      'sealed Tauri bundle does not match the exact artifacts produced by the attested build',
    );
  }
}
