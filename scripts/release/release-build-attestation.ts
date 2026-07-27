import { createHash } from 'node:crypto';
import {
  readFile,
  readlink,
  readdir,
  lstat,
  stat,
  writeFile,
} from 'node:fs/promises';
import {
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
  win32,
} from 'node:path';

import { updaterPublicKeyFingerprint } from './release-config';
import { inspectReviewedBinary } from './binary-identity';

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

interface CredentialedOutputEntry {
  path: string;
  kind: 'file' | 'symlink';
  mode: number;
  bytes: number;
  sha256: string;
}

export interface VerifiedCredentialedOutput {
  releaseConfigSha256: string;
  updaterPublicKeySha256: string;
  unsignedHandoffSha256: string;
  reviewedBinaryPath: string;
  reviewedBinarySha256: string;
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function releaseConfigKey(
  input: unknown,
  expected: { target: string; bundles: string },
): {
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
        digestAlgorithm?: unknown;
        signCommand?: {
          cmd?: unknown;
          args?: unknown;
        };
        nsis?: {
          installerHooks?: unknown;
        };
        timestampUrl?: unknown;
      };
      macOS?: {
        minimumSystemVersion?: unknown;
        signingIdentity?: unknown;
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
  const expectedPlatform =
    expected.target === 'universal-apple-darwin' && expected.bundles === 'dmg'
      ? 'macos'
      : expected.target === 'x86_64-pc-windows-msvc' &&
          expected.bundles === 'nsis'
        ? 'windows'
        : expected.target === 'x86_64-unknown-linux-gnu' &&
            expected.bundles === 'appimage,deb,rpm'
          ? 'linux'
          : undefined;
  if (!expectedPlatform) {
    throw new Error(
      'release build target and bundles do not form an exact reviewed platform tuple',
    );
  }
  const windows = config.bundle?.windows;
  const macos = config.bundle?.macOS;
  if (
    (expectedPlatform === 'macos' &&
      (windows !== undefined ||
        macos?.minimumSystemVersion !== '12.0' ||
        typeof macos.signingIdentity !== 'string' ||
        macos.signingIdentity.trim().length === 0)) ||
    (expectedPlatform === 'linux' &&
      (windows !== undefined || macos !== undefined))
  ) {
    throw new Error(
      `${expectedPlatform} release config does not match the exact reviewed platform shape`,
    );
  }
  const signCommand = config.bundle?.windows?.signCommand;
  let windowsSignerPath: string | undefined;
  if (expectedPlatform === 'windows') {
    if (
      macos !== undefined ||
      windows?.digestAlgorithm !== 'sha256' ||
      windows?.timestampUrl !== 'http://timestamp.digicert.com' ||
      typeof windows?.nsis?.installerHooks !== 'string' ||
      !isAbsolute(windows.nsis.installerHooks) ||
      !/[\\/]scripts[\\/]release[\\/]windows-minimum\.nsh$/.test(
        windows.nsis.installerHooks,
      ) ||
      signCommand === undefined ||
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
        'Windows release config does not contain the reviewed NSIS installer hook and signer path',
      );
    }
    windowsSignerPath = signCommand.args[3];
  } else if (signCommand !== undefined || windows !== undefined) {
    throw new Error(
      'non-Windows release config contains an unexpected Windows signer',
    );
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
  const parsed = releaseConfigKey(JSON.parse(body.toString('utf8')), {
    target,
    bundles,
  });
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
  const parsed = releaseConfigKey(JSON.parse(body.toString('utf8')), {
    target: expected.target,
    bundles: expected.bundles,
  });
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

async function credentialedOutputInventory(
  root: string,
  directory = root,
): Promise<CredentialedOutputEntry[]> {
  const output: CredentialedOutputEntry[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    const relativePath = relative(root, path).split(sep).join('/');
    if (relativePath === 'credentialed-output.json') continue;
    if (entry.isDirectory()) {
      output.push(...(await credentialedOutputInventory(root, path)));
      continue;
    }
    const metadata = await lstat(path);
    if (entry.isFile()) {
      const body = await readFile(path);
      output.push({
        path: relativePath,
        kind: 'file',
        mode: metadata.mode & 0o777,
        bytes: body.byteLength,
        sha256: sha256(body),
      });
    } else if (entry.isSymbolicLink()) {
      if (!relativePath.startsWith('bundle/')) {
        throw new Error(
          `credentialed output symbolic links are permitted only inside the bundle: ${relativePath}`,
        );
      }
      const rawTarget = await readlink(path);
      if (isAbsolute(rawTarget) || win32.isAbsolute(rawTarget)) {
        throw new Error(
          `credentialed output contains an absolute symbolic-link target: ${relativePath}`,
        );
      }
      const bundleRoot = resolve(root, 'bundle');
      const resolvedTarget = resolve(dirname(path), rawTarget);
      const targetWithinBundle = relative(bundleRoot, resolvedTarget);
      if (
        targetWithinBundle === '..' ||
        targetWithinBundle.startsWith(`..${sep}`) ||
        isAbsolute(targetWithinBundle)
      ) {
        throw new Error(
          `credentialed output symbolic link escapes the closed bundle root: ${relativePath}`,
        );
      }
      const target = Buffer.from(rawTarget, 'utf8');
      output.push({
        path: relativePath,
        kind: 'symlink',
        mode: metadata.mode & 0o777,
        bytes: target.byteLength,
        sha256: sha256(target),
      });
    } else {
      throw new Error(
        `credentialed output contains unsupported entry ${relativePath}`,
      );
    }
  }
  return output.sort((left, right) => left.path.localeCompare(right.path));
}

function credentialedEntry(value: unknown): value is CredentialedOutputEntry {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return false;
  }
  const entry = value as Partial<CredentialedOutputEntry>;
  return (
    typeof entry.path === 'string' &&
    entry.path.length > 0 &&
    (entry.kind === 'file' || entry.kind === 'symlink') &&
    Number.isInteger(entry.mode) &&
    typeof entry.mode === 'number' &&
    entry.mode >= 0 &&
    Number.isInteger(entry.bytes) &&
    typeof entry.bytes === 'number' &&
    entry.bytes >= 0 &&
    typeof entry.sha256 === 'string' &&
    SHA256.test(entry.sha256)
  );
}

export async function verifyCredentialedOutput(
  outputRoot: string,
  manifestPath: string,
  expected: {
    version: string;
    sourceCommit: string;
    platform: 'macos' | 'windows' | 'linux';
    target: string;
    bundles: string;
  },
): Promise<VerifiedCredentialedOutput> {
  if (
    resolve(manifestPath) !== resolve(join(outputRoot, 'credentialed-output.json'))
  ) {
    throw new Error(
      'credentialed output manifest must be the sealed root manifest',
    );
  }
  let manifest: {
    schemaVersion?: unknown;
    version?: unknown;
    sourceCommit?: unknown;
    platform?: unknown;
    target?: unknown;
    bundles?: unknown;
    unsignedHandoffSha256?: unknown;
    reviewedBinaryPath?: unknown;
    reviewedBinarySha256?: unknown;
    updaterPublicKeySha256?: unknown;
    releaseConfigSha256?: unknown;
    files?: unknown;
  };
  try {
    manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as typeof manifest;
  } catch {
    throw new Error('credentialed output manifest is not valid JSON');
  }
  const exactTuple =
    (expected.platform === 'macos' &&
      expected.target === 'universal-apple-darwin' &&
      expected.bundles === 'dmg') ||
    (expected.platform === 'windows' &&
      expected.target === 'x86_64-pc-windows-msvc' &&
      expected.bundles === 'nsis') ||
    (expected.platform === 'linux' &&
      expected.target === 'x86_64-unknown-linux-gnu' &&
      expected.bundles === 'appimage,deb,rpm');
  if (
    !exactTuple ||
    manifest.schemaVersion !== 2 ||
    manifest.version !== expected.version ||
    manifest.sourceCommit !== expected.sourceCommit ||
    manifest.platform !== expected.platform ||
    manifest.target !== expected.target ||
    manifest.bundles !== expected.bundles
  ) {
    throw new Error(
      'credentialed output does not match the exact release platform tuple',
    );
  }
  if (
    typeof manifest.unsignedHandoffSha256 !== 'string' ||
    !SHA256.test(manifest.unsignedHandoffSha256) ||
    typeof manifest.reviewedBinaryPath !== 'string' ||
    typeof manifest.reviewedBinarySha256 !== 'string' ||
    !SHA256.test(manifest.reviewedBinarySha256) ||
    typeof manifest.updaterPublicKeySha256 !== 'string' ||
    !SHA256.test(manifest.updaterPublicKeySha256) ||
    typeof manifest.releaseConfigSha256 !== 'string' ||
    !SHA256.test(manifest.releaseConfigSha256) ||
    !Array.isArray(manifest.files) ||
    !manifest.files.every(credentialedEntry)
  ) {
    throw new Error('credentialed output manifest has an invalid closed inventory');
  }
  const declared = manifest.files as CredentialedOutputEntry[];
  const reviewedBinaryName =
    expected.platform === 'windows' ? 'mdreader-app.exe' : 'mdreader-app';
  const reviewedBinaryPath = `reviewed-binary/${reviewedBinaryName}`;
  if (
    declared.length === 0 ||
    declared.some(
      (entry) =>
        entry.path !== 'release-config.json' &&
        !entry.path.startsWith('reviewed-binary/') &&
        !entry.path.startsWith('bundle/'),
    ) ||
    manifest.reviewedBinaryPath !== reviewedBinaryPath ||
    declared.filter((entry) => entry.path === 'release-config.json').length !==
      1 ||
    declared.filter((entry) => entry.path === reviewedBinaryPath).length !== 1 ||
    declared.some(
      (entry) =>
        entry.path.startsWith('reviewed-binary/') &&
        entry.path !== reviewedBinaryPath,
    ) ||
    !declared.some(
      (entry) => entry.kind === 'file' && entry.path.startsWith('bundle/'),
    )
  ) {
    throw new Error(
      'credentialed output manifest escapes the closed config, reviewed binary, and bundle handoff',
    );
  }
  const actual = await credentialedOutputInventory(outputRoot);
  if (JSON.stringify(declared) !== JSON.stringify(actual)) {
    throw new Error(
      'credentialed output inventory does not match the sealed files',
    );
  }
  const configPath = join(outputRoot, 'release-config.json');
  const reviewedBinaryBody = await readFile(
    join(outputRoot, ...reviewedBinaryPath.split('/')),
  );
  const reviewedEntry = declared.find(
    (entry) => entry.path === reviewedBinaryPath,
  )!;
  if (
    manifest.reviewedBinarySha256 !== sha256(reviewedBinaryBody) ||
    reviewedEntry.kind !== 'file' ||
    reviewedEntry.sha256 !== manifest.reviewedBinarySha256
  ) {
    throw new Error(
      'credentialed output reviewed executable does not match its unsigned handoff digest',
    );
  }
  inspectReviewedBinary(reviewedBinaryBody, {
    platform: expected.platform,
    target: expected.target,
    sourceCommit: expected.sourceCommit,
  });
  const configBody = await readFile(configPath);
  const parsed = releaseConfigKey(JSON.parse(configBody.toString('utf8')), {
    target: expected.target,
    bundles: expected.bundles,
  });
  const releaseConfigSha256 = sha256(configBody);
  const updaterPublicKeySha256 = updaterPublicKeyFingerprint(
    parsed.updaterPublicKey,
  );
  if (
    parsed.version !== expected.version ||
    manifest.releaseConfigSha256 !== releaseConfigSha256 ||
    manifest.updaterPublicKeySha256 !== updaterPublicKeySha256
  ) {
    throw new Error(
      'credentialed output config or updater identity does not match the sealed inventory',
    );
  }
  return {
    releaseConfigSha256,
    updaterPublicKeySha256,
    unsignedHandoffSha256: manifest.unsignedHandoffSha256,
    reviewedBinaryPath,
    reviewedBinarySha256: manifest.reviewedBinarySha256,
  };
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
