import { createHash } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  inspectAndroidBetaArtifact,
  inspectIosBetaArtifact,
} from './mobile-beta-inspection';
import {
  createMobileBetaManifest,
  type MobileBetaManifest,
  snapshotMobileBetaArtifact,
  writeMobileBetaManifest,
} from './mobile-beta-manifest';
import {
  uploadGooglePlayClosedBeta,
  uploadTestFlight,
} from './mobile-beta-upload';

const BUNDLETOOL_SHA256 =
  'a099cfa1543f55593bc2ed16a70a7c67fe54b1747bb7301f37fdfd6d91028e29';
const BUNDLETOOL_BYTES = 32_520_401;
const MAX_RELEASE_METADATA_BYTES = 1024 * 1024;

function options(args: string[]): Map<string, string> {
  const parsed = new Map<string, string>();
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    const value = args[index + 1];
    if (!key?.startsWith('--') || value === undefined || parsed.has(key)) {
      throw new Error(`invalid or duplicate option ${JSON.stringify(key)}`);
    }
    parsed.set(key, value);
  }
  return parsed;
}

function required(values: Map<string, string>, key: string): string {
  const value = values.get(key);
  if (!value) throw new Error(`missing required option ${key}`);
  return value;
}

function exactOptions(
  values: Map<string, string>,
  allowed: readonly string[],
): void {
  const extras = [...values.keys()].filter((key) => !allowed.includes(key));
  if (extras.length > 0) {
    throw new Error(`unsupported option(s): ${extras.join(', ')}`);
  }
}

async function verifyBundletool(path: string): Promise<void> {
  const metadata = await lstat(path);
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.size !== BUNDLETOOL_BYTES
  ) {
    throw new Error('bundletool does not match the reviewed regular-file identity');
  }
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  let sha256: string;
  try {
    const opened = await file.stat();
    requireSameMetadataFile(metadata, opened, BUNDLETOOL_BYTES);
    sha256 = createHash('sha256')
      .update(await file.readFile())
      .digest('hex');
    requireSameMetadataFile(opened, await file.stat(), BUNDLETOOL_BYTES);
  } finally {
    await file.close();
  }
  if (sha256 !== BUNDLETOOL_SHA256) {
    throw new Error('bundletool SHA-256 does not match the reviewed release');
  }
}

function requireSameMetadataFile(
  before: Stats,
  after: Stats,
  maximumBytes = MAX_RELEASE_METADATA_BYTES,
): void {
  if (
    !after.isFile() ||
    before.dev !== after.dev ||
    before.ino !== after.ino ||
    before.size !== after.size ||
    before.mtimeMs !== after.mtimeMs ||
    after.size <= 0 ||
    after.size > maximumBytes
  ) {
    throw new Error('release metadata changed or is outside reviewed bounds');
  }
}

async function readProtectedText(
  path: string,
  label: string,
): Promise<{ body: string; sha256: string }> {
  const pathIdentity = await lstat(path);
  if (
    !pathIdentity.isFile() ||
    pathIdentity.isSymbolicLink() ||
    pathIdentity.size <= 0 ||
    pathIdentity.size > MAX_RELEASE_METADATA_BYTES
  ) {
    throw new Error(`${label} must be a bounded regular file`);
  }
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await file.stat();
    requireSameMetadataFile(pathIdentity, opened);
    const bytes = await file.readFile();
    requireSameMetadataFile(opened, await file.stat());
    return {
      body: bytes.toString('utf8'),
      sha256: createHash('sha256').update(bytes).digest('hex'),
    };
  } finally {
    await file.close();
  }
}

function parseBoundManifest(
  path: string,
  body: string,
): MobileBetaManifest & { artifactPath: string } {
  const parsed: unknown = JSON.parse(body);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('mobile beta manifest is not a JSON object');
  }
  const manifest = parsed as MobileBetaManifest;
  if (
    !manifest.artifact ||
    typeof manifest.artifact !== 'object' ||
    Array.isArray(manifest.artifact) ||
    !manifest.signingIdentity ||
    typeof manifest.signingIdentity !== 'object' ||
    Array.isArray(manifest.signingIdentity) ||
    !Array.isArray(manifest.architectures)
  ) {
    throw new Error('mobile beta manifest is malformed');
  }
  const rootKeys = Object.keys(parsed as Record<string, unknown>).sort();
  const expectedRootKeys = [
    'architectures',
    'artifact',
    'buildNumber',
    'channel',
    'packageId',
    'platform',
    'schemaVersion',
    'signingIdentity',
    'sourceCommit',
    'version',
  ].sort();
  const artifactKeys = Object.keys(manifest.artifact).sort();
  const signingKeys = Object.keys(manifest.signingIdentity).sort();
  const expectedSigningKeys =
    manifest.signingIdentity.kind === 'apple-distribution'
      ? ['certificateSha256', 'kind', 'teamId']
      : ['certificateSha256', 'kind'];
  if (
    JSON.stringify(rootKeys) !== JSON.stringify(expectedRootKeys) ||
    JSON.stringify(artifactKeys) !==
      JSON.stringify(['bytes', 'fileName', 'sha256']) ||
    JSON.stringify(signingKeys) !== JSON.stringify(expectedSigningKeys) ||
    manifest.schemaVersion !== 1 ||
    !['ios', 'android'].includes(manifest.platform) ||
    !['testflight', 'google-play-closed'].includes(manifest.channel) ||
    manifest.packageId !== 'dev.mdreader.app' ||
    !/^[0-9a-f]{40}$/.test(manifest.sourceCommit) ||
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(
      manifest.version,
    ) ||
    !/^[1-9][0-9]*$/.test(manifest.buildNumber) ||
    !manifest.artifact ||
    !Number.isSafeInteger(manifest.artifact.bytes) ||
    manifest.artifact.bytes <= 0 ||
    !/^[0-9a-f]{64}$/.test(manifest.artifact.sha256) ||
    basename(manifest.artifact.fileName) !== manifest.artifact.fileName ||
    /[\\/]/.test(manifest.artifact.fileName)
  ) {
    throw new Error('mobile beta manifest is malformed or not production-authorized');
  }
  const expectedExtension = manifest.platform === 'ios' ? '.ipa' : '.aab';
  if (!manifest.artifact.fileName.toLowerCase().endsWith(expectedExtension)) {
    throw new Error('mobile beta manifest artifact does not match its platform');
  }
  if (
    (manifest.platform === 'ios' &&
      (manifest.channel !== 'testflight' ||
        manifest.architectures.length !== 1 ||
        manifest.architectures[0] !== 'arm64' ||
        manifest.signingIdentity.kind !== 'apple-distribution' ||
        !/^[A-Z0-9]{10}$/.test(manifest.signingIdentity.teamId) ||
        !/^[0-9a-f]{64}$/.test(
          manifest.signingIdentity.certificateSha256,
        ))) ||
    (manifest.platform === 'android' &&
      (manifest.channel !== 'google-play-closed' ||
        manifest.architectures.length !== 1 ||
        manifest.architectures[0] !== 'arm64-v8a' ||
        manifest.signingIdentity.kind !== 'google-play-upload' ||
        !/^[0-9a-f]{64}$/.test(
          manifest.signingIdentity.certificateSha256,
        )))
  ) {
    throw new Error('mobile beta manifest platform authority is inconsistent');
  }
  return {
    ...manifest,
    artifactPath: join(dirname(path), manifest.artifact.fileName),
  };
}

export async function readMobileBetaUploadManifest(path: string): Promise<{
  manifest: MobileBetaManifest & { artifactPath: string };
  sha256: string;
}> {
  const read = await readProtectedText(path, 'mobile beta manifest');
  return {
    manifest: parseBoundManifest(path, read.body),
    sha256: read.sha256,
  };
}

export function createMobileBetaUploadReceipt(
  manifestPath: string,
  manifestSha256: string,
  manifest: MobileBetaManifest,
  providerReceipt: unknown,
): Record<string, unknown> {
  if (!/^[0-9a-f]{64}$/.test(manifestSha256)) {
    throw new Error('mobile beta manifest receipt digest is invalid');
  }
  return {
    schemaVersion: 1,
    status: 'succeeded',
    platform: manifest.platform,
    channel: manifest.channel,
    sourceCommit: manifest.sourceCommit,
    version: manifest.version,
    buildNumber: manifest.buildNumber,
    packageId: manifest.packageId,
    artifact: manifest.artifact,
    manifest: {
      fileName: basename(manifestPath),
      sha256: manifestSha256,
    },
    providerReceipt,
  };
}

export async function runWithReservedMobileBetaReceipt<T>(
  receiptPath: string,
  manifestPath: string,
  manifestSha256: string,
  manifest: MobileBetaManifest,
  operation: () => Promise<T>,
): Promise<T> {
  const output = await open(receiptPath, 'wx', 0o600);
  const write = async (body: unknown): Promise<void> => {
    const bytes = Buffer.from(`${JSON.stringify(body, null, 2)}\n`, 'utf8');
    if (bytes.byteLength > 4 * 1024 * 1024) {
      throw new Error('mobile beta receipt exceeds the reviewed byte limit');
    }
    let offset = 0;
    while (offset < bytes.byteLength) {
      const { bytesWritten } = await output.write(
        bytes,
        offset,
        bytes.byteLength - offset,
        offset,
      );
      if (bytesWritten <= 0) {
        throw new Error('mobile beta receipt write made no progress');
      }
      offset += bytesWritten;
    }
    await output.truncate(bytes.byteLength);
    await output.sync();
  };
  try {
    await write({
      ...createMobileBetaUploadReceipt(
        manifestPath,
        manifestSha256,
        manifest,
        null,
      ),
      status: 'provider-outcome-unknown',
    });
    const parent = await open(dirname(resolve(receiptPath)), 'r');
    try {
      await parent.sync();
    } finally {
      await parent.close();
    }
    try {
      const providerReceipt = await operation();
      await write(
        createMobileBetaUploadReceipt(
          manifestPath,
          manifestSha256,
          manifest,
          providerReceipt,
        ),
      );
      return providerReceipt;
    } catch (error) {
      const failure = {
        ...createMobileBetaUploadReceipt(
          manifestPath,
          manifestSha256,
          manifest,
          null,
        ),
        status: 'failed-or-ambiguous',
        failure: {
          kind: 'provider-operation-failed-or-ambiguous',
        },
      };
      try {
        await write(failure);
      } catch (receiptError) {
        throw new AggregateError(
          [error, receiptError],
          'mobile beta provider operation failed and its reserved receipt could not be persisted',
        );
      }
      throw error;
    }
  } finally {
    await output.close();
  }
}

async function inspect(platform: 'ios' | 'android', args: string[]): Promise<void> {
  const values = options(args);
  const common = [
    '--artifact',
    '--snapshot',
    '--manifest',
    '--source-commit',
    '--version',
    '--build-number',
    '--certificate-sha256',
  ];
  exactOptions(
    values,
    platform === 'ios'
      ? [...common, '--team-id']
      : [...common, '--bundletool'],
  );
  const sourceCommit = required(values, '--source-commit');
  const version = required(values, '--version');
  const buildNumber = required(values, '--build-number');
  const certificateSha256 = required(values, '--certificate-sha256').toLowerCase();
  const snapshot = required(values, '--snapshot');
  const retained = await snapshotMobileBetaArtifact(
    required(values, '--artifact'),
    snapshot,
  );
  const evidence =
    platform === 'ios'
      ? await inspectIosBetaArtifact(snapshot, {
          sourceCommit,
          version,
          buildNumber,
          teamId: required(values, '--team-id'),
          certificateSha256,
        })
      : await (async () => {
          const bundletool = required(values, '--bundletool');
          await verifyBundletool(bundletool);
          return inspectAndroidBetaArtifact(
            snapshot,
            bundletool,
            {
              sourceCommit,
              version,
              buildNumber,
              certificateSha256,
            },
          );
        })();
  const manifest = await createMobileBetaManifest(evidence, {
    expectedSourceCommit: sourceCommit,
    expectedVersion: version,
    expectedBuildNumber: buildNumber,
    expectedAppleTeamId:
      platform === 'ios' ? required(values, '--team-id') : undefined,
    expectedSigningCertificateSha256: certificateSha256,
  });
  if (
    manifest.artifact.bytes !== retained.bytes ||
    manifest.artifact.sha256 !== retained.sha256
  ) {
    throw new Error('inspection and manifest did not consume the retained artifact');
  }
  await writeMobileBetaManifest(required(values, '--manifest'), manifest);
}

async function uploadIos(args: string[]): Promise<void> {
  const values = options(args);
  exactOptions(values, [
    '--manifest',
    '--api-key-id',
    '--issuer-id',
    '--beta-group-id',
    '--private-key',
    '--receipt',
  ]);
  const manifestPath = required(values, '--manifest');
  const { manifest, sha256: manifestSha256 } =
    await readMobileBetaUploadManifest(manifestPath);
  if (manifest.platform !== 'ios' || manifest.channel !== 'testflight') {
    throw new Error('TestFlight upload requires an iOS TestFlight manifest');
  }
  await runWithReservedMobileBetaReceipt(
    required(values, '--receipt'),
    manifestPath,
    manifestSha256,
    manifest,
    () =>
      uploadTestFlight({
        artifactPath: manifest.artifactPath,
        expectedBytes: manifest.artifact.bytes,
        expectedSha256: manifest.artifact.sha256,
        expectedVersion: manifest.version,
        expectedBuildNumber: manifest.buildNumber,
        expectedBetaGroupId: required(values, '--beta-group-id'),
        apiKeyId: required(values, '--api-key-id'),
        issuerId: required(values, '--issuer-id'),
        privateKeyPath: required(values, '--private-key'),
      }),
  );
}

async function uploadAndroid(args: string[]): Promise<void> {
  const values = options(args);
  exactOptions(values, [
    '--manifest',
    '--service-account-file',
    '--service-account-email',
    '--closed-track',
    '--version-code',
    '--release-name',
    '--receipt',
  ]);
  const manifestPath = required(values, '--manifest');
  const { manifest, sha256: manifestSha256 } =
    await readMobileBetaUploadManifest(manifestPath);
  if (
    manifest.platform !== 'android' ||
    manifest.channel !== 'google-play-closed'
  ) {
    throw new Error('Google Play upload requires an Android closed-beta manifest');
  }
  const serviceAccount = await readProtectedText(
    required(values, '--service-account-file'),
    'Google Play service account',
  );
  await runWithReservedMobileBetaReceipt(
    required(values, '--receipt'),
    manifestPath,
    manifestSha256,
    manifest,
    () =>
      uploadGooglePlayClosedBeta({
        artifactPath: manifest.artifactPath,
        expectedBytes: manifest.artifact.bytes,
        expectedSha256: manifest.artifact.sha256,
        serviceAccountJson: serviceAccount.body,
        expectedServiceAccountEmail: required(
          values,
          '--service-account-email',
        ),
        packageId: 'dev.mdreader.app',
        closedTrack: required(values, '--closed-track'),
        versionCode: required(values, '--version-code'),
        releaseName: required(values, '--release-name'),
      }),
  );
}

export async function main(argv = process.argv.slice(2)): Promise<void> {
  const [command, ...args] = argv;
  if (command === 'inspect-ios') return inspect('ios', args);
  if (command === 'inspect-android') return inspect('android', args);
  if (command === 'upload-ios') return uploadIos(args);
  if (command === 'upload-android') return uploadAndroid(args);
  throw new Error(
    'usage: mobile-beta-cli.ts <inspect-ios|inspect-android|upload-ios|upload-android> [options]',
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main().catch((error) => {
    process.stderr.write(
      `${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  });
}
