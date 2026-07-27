import { createHash } from 'node:crypto';

import type { ReleasePlatform } from '../../apps/web/lib/release-manifest';

const SOURCE_COMMIT = /^[0-9a-f]{40}$/;
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
}

export interface PackageBinaryEvidence {
  schemaVersion: 1;
  platform: ReleasePlatform;
  target: string;
  sourceCommit: string;
  product: 'MDReader';
  identifier: 'dev.mdreader.app';
  packageKinds: string[];
  packages: Array<{
    kind: string;
    binarySha256: string;
    format: 'ELF' | 'PE' | 'Mach-O';
    architectures: Array<'arm64' | 'x86_64'>;
  }>;
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
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

function inspectMachSlice(
  body: Buffer,
  offset: number,
  size: number,
  expectedCpu: number,
  sourceCommit: string,
): void {
  if (
    offset < 0 ||
    size < 40 ||
    offset > body.byteLength - size ||
    body.readUInt32LE(offset) !== 0xfeedfacf ||
    body.readUInt32LE(offset + 4) !== expectedCpu ||
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
  for (let index = 0; index < count; index += 1) {
    const entry = 8 + index * 20;
    const cpuType = body.readUInt32BE(entry);
    const parsed = architecture(cpuType);
    const offset = body.readUInt32BE(entry + 8);
    const size = body.readUInt32BE(entry + 12);
    if (
      parsed === null ||
      architectures.has(parsed) ||
      offset < 8 + count * 20 ||
      size < 40 ||
      offset > body.byteLength - size
    ) {
      throw new Error('universal Mach-O has an unsupported or duplicate slice');
    }
    inspectMachSlice(body, offset, size, cpuType, sourceCommit);
    architectures.add(parsed);
    ranges.push({ offset, end: offset + size });
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

const PACKAGE_KINDS: Record<ReleasePlatform, string[]> = {
  macos: ['dmg', 'updater'],
  windows: ['nsis', 'updater'],
  linux: ['appimage', 'deb', 'rpm', 'updater'],
};

export function verifyPackageBinarySet(
  platform: ReleasePlatform,
  packages: PackageBinary[],
  expected: Omit<ReviewedBinaryExpectation, 'platform'>,
): PackageBinaryEvidence {
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
      return {
        kind: entry.kind,
        ...inspectReviewedBinary(entry.binary, { platform, ...expected }),
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
    schemaVersion: 1,
    platform,
    target: expected.target,
    sourceCommit: expected.sourceCommit,
    product: EXPECTED_PRODUCT,
    identifier: EXPECTED_IDENTIFIER,
    packageKinds: actual,
    packages: inspected.map((entry) => ({
      kind: entry.kind,
      binarySha256: entry.sha256,
      format: entry.format,
      architectures: entry.architectures,
    })),
  };
}
