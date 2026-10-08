import { createHash } from 'node:crypto';

import type { ReleasePlatform } from './release-manifest';

const SOURCE_COMMIT = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const IDENTITY_PREFIX = Buffer.from('MDREADER_RELEASE_IDENTITY_V1\0', 'utf8');
const EXPECTED_PRODUCT = 'MDReader';
const EXPECTED_IDENTIFIER = 'dev.mdreader.app';
const CPU_X86_64 = 0x01000007;
const CPU_ARM64 = 0x0100000c;

export interface ReviewedBinaryExpectation {
  platform: ReleasePlatform;
  target: string;
  sourceCommit: string;
}

export interface ReviewedBinaryInspection {
  format: 'ELF' | 'PE' | 'Mach-O';
  architectures: Array<'arm64' | 'x86_64'>;
  product: 'MDReader';
  identifier: 'dev.mdreader.app';
  sourceCommit: string;
  sha256: string;
}

export interface PackageBinary {
  kind: string;
  binary: Uint8Array;
  artifact: Uint8Array;
  signatureAuthenticated: boolean;
  signatureIdentity: string | null;
  entrypoint: PackageEntrypoint;
}

export interface PackageEntrypoint {
  contract:
    | 'macos-bundle-executable'
    | 'windows-nsis-install-target'
    | 'linux-apprun-target'
    | 'linux-package-install-target';
  path: string;
}

export interface PackageBinaryEvidence {
  schemaVersion: 2;
  platform: ReleasePlatform;
  target: string;
  version: string;
  sourceCommit: string;
  unsignedHandoffSha256: string;
  reviewedBinarySha256: string;
  reviewedPayloadSha256: string;
  product: 'MDReader';
  identifier: 'dev.mdreader.app';
  publisherIdentity: string | null;
  packageKinds: string[];
  packages: Array<{
    kind: string;
    artifactSha256: string;
    binarySha256: string;
    payloadSha256: string;
    transformation: 'exact' | 'authenticode' | 'code-signature';
    publisherIdentity: string | null;
    entrypointContract: PackageEntrypoint['contract'];
    entrypointPath: string;
    format: 'ELF' | 'PE' | 'Mach-O';
    architectures: Array<'arm64' | 'x86_64'>;
  }>;
}

export interface PackageBinarySetExpectation
  extends Omit<ReviewedBinaryExpectation, 'platform'> {
  unsignedHandoffSha256: string;
  reviewedBinarySha256: string;
  reviewedBinary: Uint8Array;
  expectedPublisherIdentity: string | null;
  expectedVersion: string;
}

export interface FinalPackageArtifact {
  kind: string;
  artifact: Uint8Array;
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function hashParts(parts: Array<Uint8Array | string>): string {
  const hash = createHash('sha256');
  for (const part of parts) hash.update(part);
  return hash.digest('hex');
}

function expectedIdentity(sourceCommit: string): Buffer {
  return Buffer.from(
    [
      'MDREADER_RELEASE_IDENTITY_V1',
      `productName=${EXPECTED_PRODUCT}`,
      `bundleIdentifier=${EXPECTED_IDENTIFIER}`,
      `sourceCommit=${sourceCommit}`,
      '',
    ].join('\0'),
    'utf8',
  );
}

function assertIdentity(body: Buffer, sourceCommit: string): void {
  if (body.indexOf(expectedIdentity(sourceCommit)) >= 0) return;
  const prefix = body.indexOf(IDENTITY_PREFIX);
  if (prefix < 0) {
    throw new Error('executable has no reviewed MDReader identity');
  }
  const identity = body
    .subarray(prefix, Math.min(body.byteLength, prefix + 256))
    .toString('utf8');
  if (!identity.includes(`productName=${EXPECTED_PRODUCT}\0`)) {
    throw new Error('executable has the wrong MDReader product identity');
  }
  if (!identity.includes(`bundleIdentifier=${EXPECTED_IDENTIFIER}\0`)) {
    throw new Error('executable has the wrong MDReader bundle identifier');
  }
  throw new Error('executable has the wrong MDReader source commit');
}

function inspectElf(body: Buffer, sourceCommit: string): Array<'x86_64'> {
  if (
    body.byteLength < 64 ||
    !body.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])) ||
    body[4] !== 2 ||
    body[5] !== 1 ||
    body[6] !== 1 ||
    ![2, 3].includes(body.readUInt16LE(16)) ||
    body.readUInt16LE(18) !== 62 ||
    body.readUInt32LE(20) !== 1 ||
    body.readUInt16LE(52) !== 64
  ) {
    throw new Error('Linux package executable format is not ELF64 x86_64');
  }
  const programOffset = Number(body.readBigUInt64LE(32));
  const programEntrySize = body.readUInt16LE(54);
  const programCount = body.readUInt16LE(56);
  if (
    !Number.isSafeInteger(programOffset) ||
    programOffset < 64 ||
    programEntrySize < 56 ||
    programCount < 1 ||
    programOffset + programEntrySize * programCount > body.byteLength
  ) {
    throw new Error('Linux ELF64 executable has no valid program header table');
  }
  let executableLoad = false;
  for (let index = 0; index < programCount; index += 1) {
    const entry = programOffset + index * programEntrySize;
    if (body.readUInt32LE(entry) === 1 && (body.readUInt32LE(entry + 4) & 1) !== 0) {
      executableLoad = true;
      break;
    }
  }
  if (!executableLoad) {
    throw new Error('Linux ELF64 executable has no executable load segment');
  }
  assertIdentity(body, sourceCommit);
  return ['x86_64'];
}

function inspectPe(body: Buffer, sourceCommit: string): Array<'x86_64'> {
  if (body.byteLength < 64 || body[0] !== 0x4d || body[1] !== 0x5a) {
    throw new Error('Windows package executable format is not PE');
  }
  const header = body.readUInt32LE(0x3c);
  if (
    header < 64 ||
    header > body.byteLength - 24 ||
    !body.subarray(header, header + 4).equals(Buffer.from([0x50, 0x45, 0, 0]))
  ) {
    throw new Error('Windows package executable format is not PE');
  }
  const coff = header + 4;
  const sections = body.readUInt16LE(coff + 2);
  const optionalBytes = body.readUInt16LE(coff + 16);
  const characteristics = body.readUInt16LE(coff + 18);
  const optional = coff + 20;
  const sectionTable = optional + optionalBytes;
  if (
    body.readUInt16LE(coff) !== 0x8664 ||
    sections < 1 ||
    sections > 96 ||
    optionalBytes < 112 ||
    sectionTable + sections * 40 > body.byteLength ||
    body.readUInt16LE(optional) !== 0x20b ||
    (characteristics & 0x0002) === 0
  ) {
    throw new Error('Windows package executable format is not PE32+ x86_64');
  }
  let executableCode = false;
  for (let index = 0; index < sections; index += 1) {
    const entry = sectionTable + index * 40;
    const flags = body.readUInt32LE(entry + 36);
    if ((flags & 0x00000020) !== 0 && (flags & 0x20000000) !== 0) {
      executableCode = true;
      break;
    }
  }
  if (!executableCode) {
    throw new Error('Windows PE32+ executable has no executable code section');
  }
  assertIdentity(body, sourceCommit);
  return ['x86_64'];
}

function architecture(cpuType: number): 'arm64' | 'x86_64' | null {
  if (cpuType === CPU_ARM64) return 'arm64';
  if (cpuType === CPU_X86_64) return 'x86_64';
  return null;
}

function expectedMachSubtype(cpuType: number): number {
  if (cpuType === CPU_ARM64) return 0;
  if (cpuType === CPU_X86_64) return 3;
  throw new Error('universal Mach-O has an unsupported CPU type');
}

function alignMachOffset(offset: number, exponent: number): number {
  if (!Number.isInteger(exponent) || exponent < 0 || exponent > 30) {
    throw new Error('universal Mach-O slice alignment is invalid');
  }
  const alignment = 2 ** exponent;
  return Math.ceil(offset / alignment) * alignment;
}

function inspectMachSlice(
  body: Buffer,
  offset: number,
  size: number,
  expectedCpu: number,
  expectedSubtype: number,
  sourceCommit: string,
): void {
  if (
    offset < 0 ||
    size < 40 ||
    offset > body.byteLength - size ||
    body.readUInt32LE(offset) !== 0xfeedfacf ||
    body.readUInt32LE(offset + 4) !== expectedCpu ||
    body.readUInt32LE(offset + 8) !== expectedSubtype ||
    body.readUInt32LE(offset + 12) !== 2
  ) {
    throw new Error('universal Mach-O contains an invalid executable slice');
  }
  const commandCount = body.readUInt32LE(offset + 16);
  const commandBytes = body.readUInt32LE(offset + 20);
  if (
    commandCount < 1 ||
    commandBytes < 8 ||
    32 + commandBytes > size
  ) {
    throw new Error('universal Mach-O contains a malformed load-command table');
  }
  assertIdentity(body.subarray(offset, offset + size), sourceCommit);
}

function inspectMachO(
  body: Buffer,
  sourceCommit: string,
): Array<'arm64' | 'x86_64'> {
  if (body.byteLength < 48 || body.readUInt32BE(0) !== 0xcafebabe) {
    throw new Error('macOS package executable format is not universal Mach-O');
  }
  const count = body.readUInt32BE(4);
  if (count !== 2 || body.byteLength < 8 + count * 20) {
    throw new Error('universal Mach-O must contain both arm64 and x86_64 slices');
  }
  const architectures = new Set<'arm64' | 'x86_64'>();
  const ranges: Array<{ offset: number; end: number }> = [];
  let precedingEnd = 8 + count * 20;
  for (let index = 0; index < count; index += 1) {
    const entry = 8 + index * 20;
    const cpuType = body.readUInt32BE(entry);
    const parsed = architecture(cpuType);
    const cpuSubtype = body.readUInt32BE(entry + 4);
    const offset = body.readUInt32BE(entry + 8);
    const size = body.readUInt32BE(entry + 12);
    const alignment = body.readUInt32BE(entry + 16);
    const expectedOffset = alignMachOffset(precedingEnd, alignment);
    if (
      parsed === null ||
      architectures.has(parsed) ||
      cpuSubtype !== expectedMachSubtype(cpuType) ||
      offset !== expectedOffset ||
      size < 40 ||
      offset > body.byteLength - size ||
      !body.subarray(precedingEnd, offset).every((byte) => byte === 0)
    ) {
      throw new Error(
        'universal Mach-O has an unsupported, misaligned, or duplicate slice',
      );
    }
    inspectMachSlice(body, offset, size, cpuType, cpuSubtype, sourceCommit);
    architectures.add(parsed);
    ranges.push({ offset, end: offset + size });
    precedingEnd = offset + size;
  }
  ranges.sort((left, right) => left.offset - right.offset);
  if (
    ranges[0]!.end > ranges[1]!.offset ||
    !architectures.has('arm64') ||
    !architectures.has('x86_64')
  ) {
    throw new Error('universal Mach-O must contain both arm64 and x86_64 slices');
  }
  return ['arm64', 'x86_64'];
}

export function inspectReviewedBinary(
  input: Uint8Array,
  expected: ReviewedBinaryExpectation,
): ReviewedBinaryInspection {
  if (!SOURCE_COMMIT.test(expected.sourceCommit)) {
    throw new Error('reviewed binary source commit must be an exact Git SHA');
  }
  const body = Buffer.from(input);
  let format: ReviewedBinaryInspection['format'];
  let architectures: ReviewedBinaryInspection['architectures'];
  if (
    expected.platform === 'linux' &&
    expected.target === 'x86_64-unknown-linux-gnu'
  ) {
    format = 'ELF';
    architectures = inspectElf(body, expected.sourceCommit);
  } else if (
    expected.platform === 'windows' &&
    expected.target === 'x86_64-pc-windows-msvc'
  ) {
    format = 'PE';
    architectures = inspectPe(body, expected.sourceCommit);
  } else if (
    expected.platform === 'macos' &&
    expected.target === 'universal-apple-darwin'
  ) {
    format = 'Mach-O';
    architectures = inspectMachO(body, expected.sourceCommit);
  } else {
    throw new Error(
      `unsupported reviewed binary platform/target ${expected.platform}/${expected.target}`,
    );
  }
  return {
    format,
    architectures,
    product: EXPECTED_PRODUCT,
    identifier: EXPECTED_IDENTIFIER,
    sourceCommit: expected.sourceCommit,
    sha256: sha256(body),
  };
}

function pePayload(body: Buffer): {
  sha256: string;
  hasAuthenticode: boolean;
} {
  const header = body.readUInt32LE(0x3c);
  const coff = header + 4;
  const optional = coff + 20;
  const optionalBytes = body.readUInt16LE(coff + 16);
  const securityDirectory = optional + 112 + 4 * 8;
  if (optionalBytes < 152 || securityDirectory + 8 > optional + optionalBytes) {
    throw new Error('Windows PE has no complete Authenticode directory');
  }
  const certificateOffset = body.readUInt32LE(securityDirectory);
  const certificateBytes = body.readUInt32LE(securityDirectory + 4);
  if ((certificateOffset === 0) !== (certificateBytes === 0)) {
    throw new Error('Windows Authenticode directory is incomplete');
  }
  let payloadEnd = body.byteLength;
  let hasAuthenticode = false;
  if (certificateOffset !== 0) {
    if (
      certificateOffset % 8 !== 0 ||
      certificateBytes < 8 ||
      certificateOffset > body.byteLength - certificateBytes ||
      certificateOffset + certificateBytes !== body.byteLength
    ) {
      throw new Error('Windows Authenticode certificate table is not an exact trailing table');
    }
    let offset = certificateOffset;
    const end = certificateOffset + certificateBytes;
    while (offset < end) {
      if (offset > end - 8) {
        throw new Error('Windows Authenticode certificate table is truncated');
      }
      const length = body.readUInt32LE(offset);
      if (
        length < 8 ||
        offset > end - length ||
        body.readUInt16LE(offset + 4) !== 0x0200 ||
        body.readUInt16LE(offset + 6) !== 0x0002
      ) {
        throw new Error('Windows Authenticode certificate entry is invalid');
      }
      const next = Math.ceil((offset + length) / 8) * 8;
      if (
        next > end ||
        !body.subarray(offset + length, next).every((byte) => byte === 0)
      ) {
        throw new Error('Windows Authenticode certificate padding is invalid');
      }
      offset = next;
    }
    if (offset !== end) {
      throw new Error('Windows Authenticode certificate table is not closed');
    }
    payloadEnd = certificateOffset;
    hasAuthenticode = true;
  }
  const normalizedLength = Math.ceil(payloadEnd / 8) * 8;
  const normalized = Buffer.alloc(normalizedLength);
  body.copy(normalized, 0, 0, payloadEnd);
  normalized.fill(0, optional + 64, optional + 68);
  normalized.fill(0, securityDirectory, securityDirectory + 8);
  return { sha256: sha256(normalized), hasAuthenticode };
}

function machSlicePayload(
  body: Buffer,
  sliceOffset: number,
  sliceBytes: number,
): { sha256: string; hasCodeSignature: boolean; payloadBytes: number } {
  const commandCount = body.readUInt32LE(sliceOffset + 16);
  const commandBytes = body.readUInt32LE(sliceOffset + 20);
  let commandOffset = sliceOffset + 32;
  const commandEnd = commandOffset + commandBytes;
  if (
    commandCount < 1 ||
    commandEnd > sliceOffset + sliceBytes
  ) {
    throw new Error('Mach-O payload has a malformed load-command table');
  }
  let signature:
    | { commandOffset: number; dataOffset: number; dataBytes: number }
    | undefined;
  const linkeditCommands: number[] = [];
  for (let index = 0; index < commandCount; index += 1) {
    if (commandOffset > commandEnd - 8) {
      throw new Error('Mach-O payload has a truncated load command');
    }
    const command = body.readUInt32LE(commandOffset);
    const size = body.readUInt32LE(commandOffset + 4);
    if (size < 8 || commandOffset > commandEnd - size) {
      throw new Error('Mach-O payload has an invalid load command');
    }
    if (command === 0x1d) {
      if (size < 16 || signature !== undefined) {
        throw new Error('Mach-O payload has an invalid code-signature command');
      }
      signature = {
        commandOffset,
        dataOffset: body.readUInt32LE(commandOffset + 8),
        dataBytes: body.readUInt32LE(commandOffset + 12),
      };
    } else if (
      command === 0x19 &&
      size >= 72 &&
      body
        .subarray(commandOffset + 8, commandOffset + 24)
        .subarray(0, 10)
        .equals(Buffer.from('__LINKEDIT'))
    ) {
      linkeditCommands.push(commandOffset);
    }
    commandOffset += size;
  }
  if (commandOffset !== commandEnd) {
    throw new Error('Mach-O load-command count does not close its table');
  }
  let payloadEnd = sliceBytes;
  let hasCodeSignature = false;
  if (signature !== undefined) {
    if (
      signature.dataBytes < 12 ||
      signature.dataOffset < commandEnd - sliceOffset ||
      signature.dataOffset > sliceBytes - signature.dataBytes ||
      signature.dataOffset + signature.dataBytes !== sliceBytes ||
      body.readUInt32BE(sliceOffset + signature.dataOffset) !== 0xfade0cc0
    ) {
      throw new Error('Mach-O code-signature blob is not an exact trailing superblob');
    }
    payloadEnd = signature.dataOffset;
    hasCodeSignature = true;
  }
  const normalized = Buffer.from(
    body.subarray(sliceOffset, sliceOffset + payloadEnd),
  );
  if (signature !== undefined) {
    const relativeCommand = signature.commandOffset - sliceOffset;
    normalized.fill(0, relativeCommand + 8, relativeCommand + 16);
    for (const absoluteCommand of linkeditCommands) {
      const command = absoluteCommand - sliceOffset;
      normalized.fill(0, command + 32, command + 40);
      normalized.fill(0, command + 48, command + 56);
    }
  }
  return {
    sha256: sha256(normalized),
    hasCodeSignature,
    payloadBytes: normalized.byteLength,
  };
}

function machPayload(body: Buffer): {
  sha256: string;
  hasCodeSignature: boolean;
} {
  const count = body.readUInt32BE(4);
  const slices: Array<{
    architecture: 'arm64' | 'x86_64';
    cpuSubtype: number;
    alignment: number;
    sha256: string;
    hasCodeSignature: boolean;
    payloadBytes: number;
    end: number;
  }> = [];
  for (let index = 0; index < count; index += 1) {
    const entry = 8 + index * 20;
    const parsed = architecture(body.readUInt32BE(entry));
    if (parsed === null) throw new Error('Mach-O payload architecture is invalid');
    const cpuSubtype = body.readUInt32BE(entry + 4);
    const offset = body.readUInt32BE(entry + 8);
    const size = body.readUInt32BE(entry + 12);
    const alignment = body.readUInt32BE(entry + 16);
    const payload = machSlicePayload(body, offset, size);
    slices.push({
      architecture: parsed,
      cpuSubtype,
      alignment,
      ...payload,
      end: offset + size,
    });
  }
  const finalEnd = Math.max(...slices.map((slice) => slice.end));
  if (finalEnd !== body.byteLength) {
    throw new Error('universal Mach-O contains unreviewed trailing bytes');
  }
  return {
    sha256: hashParts(
      slices.flatMap((slice, index) => [
        `slice=${index}\0architecture=${slice.architecture}\0subtype=${slice.cpuSubtype}\0alignment=${slice.alignment}\0payloadBytes=${slice.payloadBytes}\0`,
        slice.sha256,
        '\0',
      ]),
    ),
    hasCodeSignature: slices.every((slice) => slice.hasCodeSignature),
  };
}

function payloadIdentity(
  platform: ReleasePlatform,
  body: Uint8Array,
): { sha256: string; authenticatedTransformation: boolean } {
  const bytes = Buffer.from(body);
  if (platform === 'windows') {
    const payload = pePayload(bytes);
    return {
      sha256: payload.sha256,
      authenticatedTransformation: payload.hasAuthenticode,
    };
  }
  if (platform === 'macos') {
    const payload = machPayload(bytes);
    return {
      sha256: payload.sha256,
      authenticatedTransformation: payload.hasCodeSignature,
    };
  }
  return { sha256: sha256(bytes), authenticatedTransformation: false };
}

const PACKAGE_KINDS: Record<ReleasePlatform, string[]> = {
  macos: ['dmg', 'updater'],
  windows: ['nsis', 'updater'],
  linux: ['appimage', 'deb', 'rpm', 'updater'],
};

function expectedEntrypoint(
  platform: ReleasePlatform,
  kind: string,
): PackageEntrypoint {
  if (platform === 'macos') {
    return {
      contract: 'macos-bundle-executable',
      path: 'MDReader.app/Contents/MacOS/mdreader-app',
    };
  }
  if (platform === 'windows') {
    return {
      contract: 'windows-nsis-install-target',
      path: 'mdreader-app.exe',
    };
  }
  if (kind === 'appimage' || kind === 'updater') {
    return {
      contract: 'linux-apprun-target',
      path: 'usr/bin/mdreader-app',
    };
  }
  return {
    contract: 'linux-package-install-target',
    path: 'usr/bin/mdreader-app',
  };
}

export function verifyPackageBinarySet(
  platform: ReleasePlatform,
  packages: PackageBinary[],
  expected: PackageBinarySetExpectation,
): PackageBinaryEvidence {
  if (!SHA256.test(expected.unsignedHandoffSha256)) {
    throw new Error('unsigned handoff digest must be an exact SHA-256');
  }
  if (!/^\d+\.\d+\.\d+$/.test(expected.expectedVersion)) {
    throw new Error('expected final package version must be an exact stable semantic version');
  }
  const reviewed = inspectReviewedBinary(expected.reviewedBinary, {
    platform,
    target: expected.target,
    sourceCommit: expected.sourceCommit,
  });
  if (
    !SHA256.test(expected.reviewedBinarySha256) ||
    reviewed.sha256 !== expected.reviewedBinarySha256
  ) {
    throw new Error('reviewed handoff executable digest does not match its exact bytes');
  }
  const reviewedPayload = payloadIdentity(platform, expected.reviewedBinary);
  if (
    (platform === 'macos' &&
      !/^[A-Z0-9]{10}$/.test(expected.expectedPublisherIdentity ?? '')) ||
    (platform === 'windows' &&
      !/^[0-9A-F]{40}$/.test(expected.expectedPublisherIdentity ?? '')) ||
    (platform === 'linux' && expected.expectedPublisherIdentity !== null)
  ) {
    throw new Error(`${platform} expected publisher identity is invalid`);
  }
  const wanted = PACKAGE_KINDS[platform];
  const actual = packages.map((entry) => entry.kind);
  if (
    actual.length !== wanted.length ||
    actual.some((kind, index) => kind !== wanted[index])
  ) {
    throw new Error(
      `${platform} final package set must be exactly ${wanted.join(', ')}`,
    );
  }
  const inspected = packages.map((entry) => {
    try {
      const entrypoint = expectedEntrypoint(platform, entry.kind);
      if (
        entry.entrypoint?.contract !== entrypoint.contract ||
        entry.entrypoint.path !== entrypoint.path
      ) {
        throw new Error(
          `package entrypoint is not bound by ${entrypoint.contract} to ${entrypoint.path}`,
        );
      }
      if (platform !== 'linux' && entry.signatureAuthenticated !== true) {
        throw new Error('platform signature was not independently authenticated');
      }
      if (entry.signatureIdentity !== expected.expectedPublisherIdentity) {
        throw new Error(
          `${platform} package publisher identity does not match the independently reviewed ${
            platform === 'macos' ? 'team identifier' : 'certificate thumbprint'
          }`,
        );
      }
      const inspection = inspectReviewedBinary(entry.binary, {
        platform,
        ...expected,
      });
      const binarySha256 = sha256(entry.binary);
      const payload = payloadIdentity(platform, entry.binary);
      const exact = binarySha256 === reviewed.sha256;
      if (
        (!exact &&
          (platform === 'linux' ||
            !payload.authenticatedTransformation ||
            payload.sha256 !== reviewedPayload.sha256)) ||
        (exact && platform === 'linux' && payload.sha256 !== reviewedPayload.sha256)
      ) {
        throw new Error(
          'executable is neither the exact reviewed handoff nor an authenticated signature-preserving payload equivalent',
        );
      }
      const transformation: PackageBinaryEvidence['packages'][number]['transformation'] =
        exact
          ? 'exact'
          : platform === 'windows'
            ? 'authenticode'
            : 'code-signature';
      return {
        kind: entry.kind,
        ...inspection,
        artifactSha256: sha256(entry.artifact),
        payloadSha256: payload.sha256,
        transformation,
        publisherIdentity: entry.signatureIdentity,
        entrypointContract: entrypoint.contract,
        entrypointPath: entrypoint.path,
      };
    } catch (error) {
      throw new Error(
        `${entry.kind} final package binary refused: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    }
  });
  return {
    schemaVersion: 2,
    platform,
    target: expected.target,
    version: expected.expectedVersion,
    sourceCommit: expected.sourceCommit,
    unsignedHandoffSha256: expected.unsignedHandoffSha256,
    reviewedBinarySha256: reviewed.sha256,
    reviewedPayloadSha256: reviewedPayload.sha256,
    product: EXPECTED_PRODUCT,
    identifier: EXPECTED_IDENTIFIER,
    publisherIdentity: expected.expectedPublisherIdentity,
    packageKinds: actual,
    packages: inspected.map((entry) => ({
      kind: entry.kind,
      artifactSha256: entry.artifactSha256,
      binarySha256: entry.sha256,
      payloadSha256: entry.payloadSha256,
      transformation: entry.transformation,
      publisherIdentity: entry.publisherIdentity,
      entrypointContract: entry.entrypointContract,
      entrypointPath: entry.entrypointPath,
      format: entry.format,
      architectures: entry.architectures,
    })),
  };
}

export function verifyFinalPackageEvidence(
  evidence: PackageBinaryEvidence,
  artifacts: FinalPackageArtifact[],
  expected: Omit<PackageBinarySetExpectation, 'expectedPublisherIdentity'> & {
    platform: ReleasePlatform;
  },
): void {
  const wanted = PACKAGE_KINDS[expected.platform];
  const actualKinds = artifacts.map((entry) => entry.kind);
  if (
    actualKinds.length !== wanted.length ||
    actualKinds.some((kind, index) => kind !== wanted[index])
  ) {
    throw new Error(
      `${expected.platform} staged package set is not the exact final package matrix`,
    );
  }
  const reviewed = inspectReviewedBinary(expected.reviewedBinary, expected);
  const reviewedPayload = payloadIdentity(
    expected.platform,
    expected.reviewedBinary,
  );
  if (evidence.sourceCommit !== expected.sourceCommit) {
    throw new Error('final package evidence has the wrong source commit');
  }
  if (evidence.version !== expected.expectedVersion) {
    throw new Error('final package evidence has the wrong release version');
  }
  if (evidence.unsignedHandoffSha256 !== expected.unsignedHandoffSha256) {
    throw new Error('final package evidence has the wrong unsigned handoff digest');
  }
  if (
    evidence.schemaVersion !== 2 ||
    evidence.platform !== expected.platform ||
    evidence.target !== expected.target ||
    evidence.product !== EXPECTED_PRODUCT ||
    evidence.identifier !== EXPECTED_IDENTIFIER ||
    (expected.platform === 'macos' &&
      !/^[A-Z0-9]{10}$/.test(evidence.publisherIdentity ?? '')) ||
    (expected.platform === 'windows' &&
      !/^[0-9A-F]{40}$/.test(evidence.publisherIdentity ?? '')) ||
    (expected.platform === 'linux' && evidence.publisherIdentity !== null) ||
    evidence.reviewedBinarySha256 !== expected.reviewedBinarySha256 ||
    evidence.reviewedBinarySha256 !== reviewed.sha256 ||
    evidence.reviewedPayloadSha256 !== reviewedPayload.sha256 ||
    JSON.stringify(evidence.packageKinds) !== JSON.stringify(wanted) ||
    !Array.isArray(evidence.packages) ||
    evidence.packages.length !== wanted.length
  ) {
    throw new Error('final package evidence does not match the reviewed handoff identity');
  }
  for (let index = 0; index < wanted.length; index += 1) {
    const kind = wanted[index]!;
    const recorded = evidence.packages[index]!;
    const artifact = artifacts[index]!;
    if (
      recorded.kind !== kind ||
      artifact.kind !== kind ||
      !SHA256.test(recorded.artifactSha256) ||
      recorded.artifactSha256 !== sha256(artifact.artifact)
    ) {
      throw new Error(`${kind} package substitution detected before staging`);
    }
    if (
      !SHA256.test(recorded.binarySha256) ||
      recorded.payloadSha256 !== reviewedPayload.sha256 ||
      recorded.entrypointContract !==
        expectedEntrypoint(expected.platform, kind).contract ||
      recorded.entrypointPath !==
        expectedEntrypoint(expected.platform, kind).path ||
      recorded.publisherIdentity !== evidence.publisherIdentity ||
      !['exact', 'authenticode', 'code-signature'].includes(
        recorded.transformation,
      )
    ) {
      throw new Error(`${kind} final package payload evidence is invalid`);
    }
    const expectedFormat =
      expected.platform === 'linux'
        ? 'ELF'
        : expected.platform === 'windows'
          ? 'PE'
          : 'Mach-O';
    const expectedArchitectures =
      expected.platform === 'macos'
        ? ['arm64', 'x86_64']
        : ['x86_64'];
    const allowedTransformations =
      expected.platform === 'linux'
        ? ['exact']
        : expected.platform === 'windows'
          ? ['exact', 'authenticode']
          : ['exact', 'code-signature'];
    if (
      recorded.format !== expectedFormat ||
      JSON.stringify(recorded.architectures) !==
        JSON.stringify(expectedArchitectures) ||
      !allowedTransformations.includes(recorded.transformation) ||
      (recorded.transformation === 'exact') !==
        (recorded.binarySha256 === reviewed.sha256)
    ) {
      throw new Error(`${kind} signature-preserving transformation evidence is invalid`);
    }
    if (
      expected.platform === 'linux' &&
      (recorded.transformation !== 'exact' ||
        recorded.binarySha256 !== reviewed.sha256)
    ) {
      throw new Error(`${kind} does not contain the exact reviewed Linux executable`);
    }
  }
}
