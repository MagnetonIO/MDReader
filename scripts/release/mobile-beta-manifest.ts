import { createHash } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import { lstat, open, type FileHandle, unlink } from 'node:fs/promises';
import { basename, extname } from 'node:path';

export const MOBILE_PRODUCTION_PACKAGE_ID = 'dev.mdreader.app';

export type MobileBetaPlatform = 'ios' | 'android';
export type MobileBetaChannel = 'testflight' | 'google-play-closed';

export type MobileBetaSigningIdentity =
  | {
      kind: 'apple-distribution';
      teamId: string;
      certificateSha256: string;
    }
  | {
      kind: 'google-play-upload';
      certificateSha256: string;
    };

export interface MobileBetaIosAuthority {
  provisioningProfileUuid: string;
  provisioningProfileSha256: string;
  profileEntitlementsSha256: string;
  signedEntitlementsSha256: string;
}

export interface MobileBetaAndroidAuthority {
  googlePlayTrack: string;
}

export interface MobileBetaArtifactEvidence {
  platform: MobileBetaPlatform;
  channel: MobileBetaChannel;
  artifactPath: string;
  sourceCommit: string;
  version: string;
  buildNumber: string;
  packageId: string;
  architectures: string[];
  signingIdentity: MobileBetaSigningIdentity;
  iosAuthority?: MobileBetaIosAuthority;
  androidAuthority?: MobileBetaAndroidAuthority;
}

export interface MobileBetaExpectedAuthority {
  expectedSourceCommit: string;
  expectedVersion: string;
  expectedBuildNumber: string;
  expectedAppleTeamId?: string;
  expectedSigningCertificateSha256: string;
  expectedGooglePlayTrack?: string;
}

export interface MobileBetaManifest {
  schemaVersion: 2;
  platform: MobileBetaPlatform;
  channel: MobileBetaChannel;
  sourceCommit: string;
  version: string;
  buildNumber: string;
  packageId: typeof MOBILE_PRODUCTION_PACKAGE_ID;
  architectures: string[];
  signingIdentity: MobileBetaSigningIdentity;
  iosAuthority?: MobileBetaIosAuthority;
  androidAuthority?: MobileBetaAndroidAuthority;
  artifact: {
    fileName: string;
    bytes: number;
    sha256: string;
  };
}

export interface MobileBetaArtifactSnapshot {
  path: string;
  bytes: number;
  sha256: string;
}

export interface OpenedMobileBetaArtifact {
  handle: FileHandle;
  identity: Stats;
}

const COPY_BUFFER_BYTES = 1024 * 1024;
const MAX_MOBILE_ARTIFACT_BYTES = 4 * 1024 * 1024 * 1024;

function requireMatch(
  actual: string,
  expected: string,
  field: string,
): void {
  if (actual !== expected) {
    throw new Error(`${field} ${JSON.stringify(actual)} does not match reviewed authority`);
  }
}

function requireLowerHex(value: string, length: number, field: string): void {
  if (
    value.length !== length ||
    !value.split('').every((character) => /[0-9a-f]/.test(character))
  ) {
    throw new Error(`${field} must be ${length} lowercase hexadecimal characters`);
  }
}

function validateCommon(
  evidence: MobileBetaArtifactEvidence,
  authority: MobileBetaExpectedAuthority,
): void {
  requireLowerHex(authority.expectedSourceCommit, 40, 'expected source commit');
  requireLowerHex(evidence.sourceCommit, 40, 'artifact source commit');
  requireMatch(
    evidence.sourceCommit,
    authority.expectedSourceCommit,
    'artifact source commit',
  );

  if (!/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(authority.expectedVersion)) {
    throw new Error('expected version must be an exact stable semantic version');
  }
  requireMatch(evidence.version, authority.expectedVersion, 'artifact version');
  if (!/^[1-9][0-9]*$/.test(authority.expectedBuildNumber)) {
    throw new Error('expected build number must be a positive decimal integer');
  }
  requireMatch(
    evidence.buildNumber,
    authority.expectedBuildNumber,
    'artifact build number',
  );
  requireMatch(
    evidence.packageId,
    MOBILE_PRODUCTION_PACKAGE_ID,
    'artifact package identifier',
  );
  if (evidence.packageId.endsWith('.e2e')) {
    throw new Error('an E2E package cannot become a mobile beta candidate');
  }

  requireLowerHex(
    authority.expectedSigningCertificateSha256,
    64,
    'expected signing certificate SHA-256',
  );
  requireLowerHex(
    evidence.signingIdentity.certificateSha256,
    64,
    'artifact signing certificate SHA-256',
  );
  requireMatch(
    evidence.signingIdentity.certificateSha256,
    authority.expectedSigningCertificateSha256,
    'artifact signing certificate SHA-256',
  );
}

function validatePlatform(evidence: MobileBetaArtifactEvidence): void {
  if (evidence.platform === 'ios') {
    requireMatch(evidence.channel, 'testflight', 'iOS beta channel');
    if (
      evidence.architectures.length !== 1 ||
      evidence.architectures[0] !== 'arm64'
    ) {
      throw new Error('iOS beta architecture must be exactly arm64');
    }
    if (extname(evidence.artifactPath).toLowerCase() !== '.ipa') {
      throw new Error('an iOS beta candidate must be an IPA');
    }
    if (evidence.signingIdentity.kind !== 'apple-distribution') {
      throw new Error('an iOS beta candidate must use an Apple distribution signer');
    }
    const iosAuthority = evidence.iosAuthority;
    if (
      !iosAuthority ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
        iosAuthority.provisioningProfileUuid,
      ) ||
      ![
        iosAuthority.provisioningProfileSha256,
        iosAuthority.profileEntitlementsSha256,
        iosAuthority.signedEntitlementsSha256,
      ].every((digest) => /^[0-9a-f]{64}$/.test(digest))
    ) {
      throw new Error('iOS profile and entitlement authority is missing or malformed');
    }
    return;
  }

  requireMatch(
    evidence.channel,
    'google-play-closed',
    'Android beta channel',
  );
  if (
    evidence.architectures.length !== 1 ||
    evidence.architectures[0] !== 'arm64-v8a'
  ) {
    throw new Error('Android beta architecture must be exactly arm64-v8a');
  }
  if (extname(evidence.artifactPath).toLowerCase() !== '.aab') {
    throw new Error('an Android beta candidate must be an AAB');
  }
  if (evidence.signingIdentity.kind !== 'google-play-upload') {
    throw new Error('an Android beta candidate must use the Google Play upload signer');
  }
}

function validateGooglePlayTrack(value: string | undefined): string {
  if (
    !value ||
    !/^[a-z][a-z0-9-]{2,63}$/.test(value) ||
    new Set(['production', 'beta', 'alpha', 'qa', 'internal']).has(value)
  ) {
    throw new Error('reviewed Google Play closed track is missing or malformed');
  }
  return value;
}

/**
 * Builds the immutable manifest for one already-inspected, signed mobile
 * package. The artifact itself is hashed after identity checks so the manifest
 * cannot be reused for different bytes.
 */
export async function createMobileBetaManifest(
  evidence: MobileBetaArtifactEvidence,
  authority: MobileBetaExpectedAuthority,
  openedArtifact?: OpenedMobileBetaArtifact,
): Promise<MobileBetaManifest> {
  validateCommon(evidence, authority);
  validatePlatform(evidence);
  if (evidence.platform === 'ios') {
    const expectedTeam = authority.expectedAppleTeamId;
    if (!expectedTeam || !/^[A-Z0-9]{10}$/.test(expectedTeam)) {
      throw new Error('reviewed Apple team identifier is missing or invalid');
    }
    requireMatch(
      evidence.signingIdentity.kind === 'apple-distribution'
        ? evidence.signingIdentity.teamId
        : '',
      expectedTeam,
      'artifact Apple team identifier',
    );
  }
  const googlePlayTrack =
    evidence.platform === 'android'
      ? validateGooglePlayTrack(authority.expectedGooglePlayTrack)
      : undefined;

  let identity: MobileBetaArtifactSnapshot;
  if (openedArtifact) {
    identity = await hashOpenedMobileBetaArtifact(
      openedArtifact.handle,
      openedArtifact.identity,
    );
  } else {
    const pathIdentity = await lstat(evidence.artifactPath);
    if (!pathIdentity.isFile() || pathIdentity.isSymbolicLink()) {
      throw new Error('mobile beta artifact must be a regular, non-symlink file');
    }
    const artifact = await open(
      evidence.artifactPath,
      constants.O_RDONLY | constants.O_NOFOLLOW,
    );
    try {
      const openedIdentity = await artifact.stat();
      requireSameFileIdentity(pathIdentity, openedIdentity);
      identity = await hashOpenedMobileBetaArtifact(artifact, openedIdentity);
    } finally {
      await artifact.close();
    }
  }
  if (identity.bytes <= 0) {
    throw new Error('mobile beta artifact is empty');
  }

  return {
    schemaVersion: 2,
    platform: evidence.platform,
    channel: evidence.channel,
    sourceCommit: evidence.sourceCommit,
    version: evidence.version,
    buildNumber: evidence.buildNumber,
    packageId: MOBILE_PRODUCTION_PACKAGE_ID,
    architectures: [...evidence.architectures],
    signingIdentity: { ...evidence.signingIdentity },
    ...(evidence.platform === 'ios'
      ? { iosAuthority: { ...evidence.iosAuthority! } }
      : {
          androidAuthority: {
            googlePlayTrack: googlePlayTrack!,
          },
        }),
    artifact: {
      fileName: basename(evidence.artifactPath),
      bytes: identity.bytes,
      sha256: identity.sha256,
    },
  };
}

function requireSameFileIdentity(before: Stats, after: Stats): void {
  if (
    !after.isFile() ||
    before.dev !== after.dev ||
    before.ino !== after.ino ||
    before.size !== after.size ||
    before.mtimeMs !== after.mtimeMs
  ) {
    throw new Error('mobile beta artifact changed while it was being retained');
  }
  if (after.size <= 0 || after.size > MAX_MOBILE_ARTIFACT_BYTES) {
    throw new Error('mobile beta artifact size is outside the accepted bounds');
  }
}

async function readOpenedArtifact(
  source: FileHandle,
  openedIdentity: Stats,
  writeChunk?: (chunk: Buffer) => Promise<void>,
): Promise<{ bytes: number; sha256: string }> {
  if (!openedIdentity.isFile()) {
    throw new Error('opened mobile beta artifact is not a regular file');
  }
  if (
    openedIdentity.size <= 0 ||
    openedIdentity.size > MAX_MOBILE_ARTIFACT_BYTES
  ) {
    throw new Error('opened mobile beta artifact size is outside the accepted bounds');
  }
  const digest = createHash('sha256');
  const buffer = Buffer.allocUnsafe(COPY_BUFFER_BYTES);
  let offset = 0;
  while (offset < openedIdentity.size) {
    const wanted = Math.min(buffer.byteLength, openedIdentity.size - offset);
    const { bytesRead } = await source.read(buffer, 0, wanted, offset);
    if (bytesRead <= 0) {
      throw new Error('mobile beta artifact ended before its opened size');
    }
    const chunk = buffer.subarray(0, bytesRead);
    digest.update(chunk);
    if (writeChunk) await writeChunk(chunk);
    offset += bytesRead;
  }
  requireSameFileIdentity(openedIdentity, await source.stat());
  return { bytes: offset, sha256: digest.digest('hex') };
}

async function hashOpenedMobileBetaArtifact(
  source: FileHandle,
  openedIdentity: Stats,
): Promise<MobileBetaArtifactSnapshot> {
  const identity = await readOpenedArtifact(source, openedIdentity);
  return { path: '', ...identity };
}

/**
 * Retains immutable candidate bytes from one already-open descriptor. Renaming
 * or replacing the source path after open cannot change the copied or hashed
 * bytes.
 */
export async function snapshotOpenedMobileBetaArtifact(
  source: FileHandle,
  openedIdentity: Stats,
  snapshotPath: string,
): Promise<MobileBetaArtifactSnapshot> {
  const destination = await open(snapshotPath, 'wx', 0o600);
  let complete = false;
  try {
    const identity = await readOpenedArtifact(
      source,
      openedIdentity,
      async (chunk) => {
        let offset = 0;
        while (offset < chunk.byteLength) {
          const { bytesWritten } = await destination.write(
            chunk,
            offset,
            chunk.byteLength - offset,
          );
          if (bytesWritten <= 0) {
            throw new Error('mobile beta snapshot write made no progress');
          }
          offset += bytesWritten;
        }
      },
    );
    await destination.sync();
    complete = true;
    return { path: snapshotPath, ...identity };
  } finally {
    await destination.close();
    if (!complete) {
      await unlink(snapshotPath).catch(() => undefined);
    }
  }
}

/**
 * Opens a source without following symlinks and retains it to a new immutable
 * path. Inspection and manifest creation must both consume this returned path.
 */
export async function snapshotMobileBetaArtifact(
  sourcePath: string,
  snapshotPath: string,
): Promise<MobileBetaArtifactSnapshot> {
  const pathIdentity = await lstat(sourcePath);
  if (!pathIdentity.isFile() || pathIdentity.isSymbolicLink()) {
    throw new Error('mobile beta source must be a regular, non-symlink file');
  }
  const source = await open(
    sourcePath,
    constants.O_RDONLY | constants.O_NOFOLLOW,
  );
  try {
    const openedIdentity = await source.stat();
    requireSameFileIdentity(pathIdentity, openedIdentity);
    return await snapshotOpenedMobileBetaArtifact(
      source,
      openedIdentity,
      snapshotPath,
    );
  } finally {
    await source.close();
  }
}

/** Writes a new manifest atomically with create-only semantics. */
export async function writeMobileBetaManifest(
  path: string,
  manifest: MobileBetaManifest,
): Promise<void> {
  const file = await open(path, 'wx', 0o600);
  try {
    await file.writeFile(`${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
    await file.sync();
  } finally {
    await file.close();
  }
}
