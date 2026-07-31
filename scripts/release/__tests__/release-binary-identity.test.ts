import { createHash } from 'node:crypto';

import { describe, expect, test } from 'vitest';

import {
  inspectReviewedBinary,
  verifyFinalPackageEvidence,
  verifyPackageBinarySet,
  type PackageBinary,
} from '../binary-identity';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const HANDOFF_SHA256 = 'a'.repeat(64);
const IDENTITY = Buffer.from(
  [
    'MDREADER_RELEASE_IDENTITY_V1',
    'productName=MDReader',
    'bundleIdentifier=dev.mdreader.app',
    `sourceCommit=${SOURCE_COMMIT}`,
    '',
  ].join('\0'),
  'utf8',
);

function elf(): Buffer {
  const body = Buffer.alloc(120);
  body.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0);
  body.writeUInt16LE(3, 16);
  body.writeUInt16LE(62, 18);
  body.writeUInt32LE(1, 20);
  body.writeBigUInt64LE(64n, 32);
  body.writeUInt16LE(64, 52);
  body.writeUInt16LE(56, 54);
  body.writeUInt16LE(1, 56);
  body.writeUInt32LE(1, 64);
  body.writeUInt32LE(5, 68);
  return Buffer.concat([body, IDENTITY]);
}

function pe(): Buffer {
  const body = Buffer.alloc(512);
  const header = 0x80;
  const coff = header + 4;
  const optional = coff + 20;
  const section = optional + 0xf0;
  body.set([0x4d, 0x5a], 0);
  body.writeUInt32LE(header, 0x3c);
  body.set([0x50, 0x45, 0, 0], header);
  body.writeUInt16LE(0x8664, coff);
  body.writeUInt16LE(1, coff + 2);
  body.writeUInt16LE(0xf0, coff + 16);
  body.writeUInt16LE(0x0002, coff + 18);
  body.writeUInt16LE(0x20b, optional);
  body.writeUInt32LE(0x20000020, section + 36);
  return Buffer.concat([body, IDENTITY]);
}

function authenticodeSignedPe(reviewed: Buffer, signatureByte: number): Buffer {
  const certificateOffset = Math.ceil(reviewed.byteLength / 8) * 8;
  const body = Buffer.alloc(certificateOffset + 16);
  reviewed.copy(body);
  const header = body.readUInt32LE(0x3c);
  const optional = header + 4 + 20;
  const securityDirectory = optional + 112 + 4 * 8;
  body.writeUInt32LE(0x12345678, optional + 64);
  body.writeUInt32LE(certificateOffset, securityDirectory);
  body.writeUInt32LE(16, securityDirectory + 4);
  body.writeUInt32LE(12, certificateOffset);
  body.writeUInt16LE(0x0200, certificateOffset + 4);
  body.writeUInt16LE(0x0002, certificateOffset + 6);
  body.fill(signatureByte, certificateOffset + 8, certificateOffset + 12);
  return body;
}

function machSlice(cpuType: number): Buffer {
  const body = Buffer.alloc(40);
  body.writeUInt32LE(0xfeedfacf, 0);
  body.writeUInt32LE(cpuType, 4);
  body.writeUInt32LE(cpuType === 0x0100000c ? 0 : 3, 8);
  body.writeUInt32LE(2, 12);
  body.writeUInt32LE(1, 16);
  body.writeUInt32LE(8, 20);
  body.writeUInt32LE(0x1b, 32);
  body.writeUInt32LE(8, 36);
  return Buffer.concat([body, IDENTITY]);
}

function universalMachO(
  architectures: Array<'arm64' | 'x86_64'> = ['arm64', 'x86_64'],
): Buffer {
  const slices = architectures.map((name) => ({
    cpu: name === 'arm64' ? 0x0100000c : 0x01000007,
    body: machSlice(name === 'arm64' ? 0x0100000c : 0x01000007),
  }));
  const header = Buffer.alloc(8 + slices.length * 20);
  header.writeUInt32BE(0xcafebabe, 0);
  header.writeUInt32BE(slices.length, 4);
  let offset = header.byteLength;
  slices.forEach((slice, index) => {
    const entry = 8 + index * 20;
    header.writeUInt32BE(slice.cpu, entry);
    header.writeUInt32BE(slice.cpu === 0x0100000c ? 0 : 3, entry + 4);
    header.writeUInt32BE(offset, entry + 8);
    header.writeUInt32BE(slice.body.byteLength, entry + 12);
    offset += slice.body.byteLength;
  });
  return Buffer.concat([header, ...slices.map((slice) => slice.body)]);
}

function signedUniversalMachO(signatureByte: number): Buffer {
  const slices = [0x0100000c, 0x01000007].map((cpu) => {
    const commands = Buffer.alloc(24);
    commands.writeUInt32LE(0x1b, 0);
    commands.writeUInt32LE(8, 4);
    commands.writeUInt32LE(0x1d, 8);
    commands.writeUInt32LE(16, 12);
    const prefix = Buffer.alloc(32);
    prefix.writeUInt32LE(0xfeedfacf, 0);
    prefix.writeUInt32LE(cpu, 4);
    prefix.writeUInt32LE(cpu === 0x0100000c ? 0 : 3, 8);
    prefix.writeUInt32LE(2, 12);
    prefix.writeUInt32LE(2, 16);
    prefix.writeUInt32LE(commands.byteLength, 20);
    const payload = Buffer.concat([prefix, commands, IDENTITY]);
    commands.writeUInt32LE(payload.byteLength, 16);
    commands.writeUInt32LE(16, 20);
    const signature = Buffer.alloc(16, signatureByte);
    signature.writeUInt32BE(0xfade0cc0, 0);
    signature.writeUInt32BE(16, 4);
    signature.writeUInt32BE(0, 8);
    return { cpu, body: Buffer.concat([prefix, commands, IDENTITY, signature]) };
  });
  const header = Buffer.alloc(48);
  header.writeUInt32BE(0xcafebabe, 0);
  header.writeUInt32BE(2, 4);
  let offset = header.byteLength;
  slices.forEach((slice, index) => {
    const entry = 8 + index * 20;
    header.writeUInt32BE(slice.cpu, entry);
    header.writeUInt32BE(slice.cpu === 0x0100000c ? 0 : 3, entry + 4);
    header.writeUInt32BE(offset, entry + 8);
    header.writeUInt32BE(slice.body.byteLength, entry + 12);
    offset += slice.body.byteLength;
  });
  return Buffer.concat([header, ...slices.map((slice) => slice.body)]);
}

function digest(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function expected(reviewedBinary: Buffer, target: string) {
  const expectedPublisherIdentity = target.includes('apple')
    ? '7727LYTG96'
    : target.includes('windows')
      ? '0123456789ABCDEF0123456789ABCDEF01234567'
      : null;
  return {
    target,
    sourceCommit: SOURCE_COMMIT,
    unsignedHandoffSha256: HANDOFF_SHA256,
    reviewedBinarySha256: digest(reviewedBinary),
    reviewedBinary,
    expectedPublisherIdentity,
    expectedVersion: '1.0.0',
  };
}

function packageSet(kinds: readonly string[], binary: Buffer): PackageBinary[] {
  const platform =
    kinds.includes('dmg') ? 'macos' : kinds.includes('nsis') ? 'windows' : 'linux';
  return kinds.map((kind) => ({
    kind,
    binary,
    artifact: Buffer.from(`final-${kind}-package`),
    signatureAuthenticated: true,
    signatureIdentity:
      platform === 'macos'
        ? '7727LYTG96'
        : platform === 'windows'
          ? '0123456789ABCDEF0123456789ABCDEF01234567'
          : null,
    entrypoint: {
      contract:
        platform === 'macos'
          ? 'macos-bundle-executable'
          : platform === 'windows'
            ? 'windows-nsis-install-target'
            : kind === 'appimage' || kind === 'updater'
              ? 'linux-apprun-target'
              : 'linux-package-install-target',
      path:
        platform === 'macos'
          ? 'MDReader.app/Contents/MacOS/mdreader-app'
          : platform === 'windows'
            ? 'mdreader-app.exe'
            : 'usr/bin/mdreader-app',
    },
  }));
}

describe('reviewed final MDReader binary identity', () => {
  test.each([
    ['linux', 'x86_64-unknown-linux-gnu', elf(), 'ELF', ['x86_64']],
    ['windows', 'x86_64-pc-windows-msvc', pe(), 'PE', ['x86_64']],
    [
      'macos',
      'universal-apple-darwin',
      universalMachO(),
      'Mach-O',
      ['arm64', 'x86_64'],
    ],
  ] as const)(
    'accepts a structured %s executable with exact target, source, and product identity',
    (platform, target, body, format, architectures) => {
      expect(
        inspectReviewedBinary(body, {
          platform,
          target,
          sourceCommit: SOURCE_COMMIT,
        }),
      ).toEqual({
        format,
        architectures,
        product: 'MDReader',
        identifier: 'dev.mdreader.app',
        sourceCommit: SOURCE_COMMIT,
        sha256: createHash('sha256').update(body).digest('hex'),
      });
    },
  );

  test.each([
    ['CPU subtype', 8 + 4, 7],
    ['slice alignment', 8 + 16, 4],
  ])(
    'does not erase the universal Mach-O %s from payload identity',
    (_label, offset, value) => {
      const reviewed = signedUniversalMachO(0x41);
      const changed = Buffer.from(signedUniversalMachO(0x42));
      changed.writeUInt32BE(value, offset);
      expect(() =>
        verifyPackageBinarySet(
          'macos',
          packageSet(['dmg', 'updater'], changed),
          expected(reviewed, 'universal-apple-darwin'),
        ),
      ).toThrow(/Mach-O|payload|slice|subtype|alignment/i);
    },
  );

  test('rejects an inserted universal Mach-O inter-slice gap', () => {
    const reviewed = signedUniversalMachO(0x41);
    const changed = signedUniversalMachO(0x42);
    const firstOffset = changed.readUInt32BE(8 + 8);
    const firstSize = changed.readUInt32BE(8 + 12);
    const secondEntry = 8 + 20;
    const secondOffset = changed.readUInt32BE(secondEntry + 8);
    const withGap = Buffer.concat([
      changed.subarray(0, secondOffset),
      Buffer.alloc(8),
      changed.subarray(secondOffset),
    ]);
    withGap.writeUInt32BE(secondOffset + 8, secondEntry + 8);
    expect(firstOffset + firstSize).toBe(secondOffset);
    expect(() =>
      verifyPackageBinarySet(
        'macos',
        packageSet(['dmg', 'updater'], withGap),
        expected(reviewed, 'universal-apple-darwin'),
      ),
    ).toThrow(/Mach-O|gap|alignment|payload/i);
  });

  test.each([
    ['shell placeholder', Buffer.concat([Buffer.from('#!/bin/sh\nexit 0\n'), IDENTITY])],
    ['executable placeholder', Buffer.concat([Buffer.from('MZplaceholder'), IDENTITY])],
    [
      'wrong product',
      Buffer.from(
        elf().toString('binary').replace('productName=MDReader', 'productName=NotMine!'),
        'binary',
      ),
    ],
    [
      'wrong source',
      Buffer.from(
        elf().toString('binary').replace(SOURCE_COMMIT, 'f'.repeat(40)),
        'binary',
      ),
    ],
  ])('rejects a deliberate %s', (_label, body) => {
    expect(() =>
      inspectReviewedBinary(body, {
        platform: 'linux',
        target: 'x86_64-unknown-linux-gnu',
        sourceCommit: SOURCE_COMMIT,
      }),
    ).toThrow(/executable format|MDReader identity|product|source commit/i);
  });

  test('rejects a universal Mach-O missing either required slice', () => {
    expect(() =>
      inspectReviewedBinary(universalMachO(['x86_64']), {
        platform: 'macos',
        target: 'universal-apple-darwin',
        sourceCommit: SOURCE_COMMIT,
      }),
    ).toThrow(/arm64.*x86_64|universal/i);
  });

  test.each([
    ['macos', 'universal-apple-darwin', ['dmg', 'updater'], universalMachO()],
    ['windows', 'x86_64-pc-windows-msvc', ['nsis', 'updater'], pe()],
    [
      'linux',
      'x86_64-unknown-linux-gnu',
      ['appimage', 'deb', 'rpm', 'updater'],
      elf(),
    ],
  ] as const)(
    'requires every final %s package to contain a reviewed executable',
    (platform, target, kinds, binary) => {
      const evidence = verifyPackageBinarySet(
        platform,
        packageSet(kinds, binary),
        expected(binary, target),
      );
      expect(evidence.packageKinds).toEqual(kinds);
      expect(evidence.packages).toHaveLength(kinds.length);
      expect(evidence.unsignedHandoffSha256).toBe(HANDOFF_SHA256);
      expect(evidence.reviewedBinarySha256).toBe(digest(binary));
      expect(
        evidence.packages.every((entry) => /^[0-9a-f]{64}$/.test(entry.binarySha256)),
      ).toBe(true);
    },
  );

  test('fails closed when one final package contains a shell placeholder', () => {
    const binary = elf();
    expect(() =>
      verifyPackageBinarySet(
        'linux',
        [
          {
            kind: 'appimage',
            binary,
            artifact: Buffer.from('appimage'),
            signatureAuthenticated: true,
            signatureIdentity: null,
            entrypoint: {
              contract: 'linux-apprun-target',
              path: 'usr/bin/mdreader-app',
            },
          },
          {
            kind: 'deb',
            binary: Buffer.from('#!/bin/sh\nexit 0\n'),
            artifact: Buffer.from('deb'),
            signatureAuthenticated: true,
            signatureIdentity: null,
            entrypoint: {
              contract: 'linux-package-install-target',
              path: 'usr/bin/mdreader-app',
            },
          },
          {
            kind: 'rpm',
            binary,
            artifact: Buffer.from('rpm'),
            signatureAuthenticated: true,
            signatureIdentity: null,
            entrypoint: {
              contract: 'linux-package-install-target',
              path: 'usr/bin/mdreader-app',
            },
          },
          {
            kind: 'updater',
            binary,
            artifact: Buffer.from('updater'),
            signatureAuthenticated: true,
            signatureIdentity: null,
            entrypoint: {
              contract: 'linux-apprun-target',
              path: 'usr/bin/mdreader-app',
            },
          },
        ],
        expected(binary, 'x86_64-unknown-linux-gnu'),
      ),
    ).toThrow(/deb.*executable format|executable format.*deb/i);
  });

  test.each([
    [
      'windows',
      'x86_64-pc-windows-msvc',
      ['nsis', 'updater'],
      pe(),
      (body: Buffer) => authenticodeSignedPe(body, 0x5a),
      'authenticode',
    ],
    [
      'macos',
      'universal-apple-darwin',
      ['dmg', 'updater'],
      signedUniversalMachO(0x41),
      () => signedUniversalMachO(0x42),
      'code-signature',
    ],
  ] as const)(
    'accepts only an independently authenticated %s signature-preserving transform',
    (platform, target, kinds, reviewed, sign, transformation) => {
      const signed = sign(reviewed);
      const evidence = verifyPackageBinarySet(
        platform,
        packageSet(kinds, signed),
        expected(reviewed, target),
      );
      expect(evidence.packages.every((entry) => entry.transformation === transformation)).toBe(
        true,
      );
      expect(
        evidence.packages.every(
          (entry) => entry.payloadSha256 === evidence.reviewedPayloadSha256,
        ),
      ).toBe(true);
      expect(
        evidence.packages.every(
          (entry) => entry.binarySha256 !== evidence.reviewedBinarySha256,
        ),
      ).toBe(true);
    },
  );

  test('rejects a forged plaintext identity marker in a different valid ELF', () => {
    const reviewed = elf();
    const forged = Buffer.concat([elf(), Buffer.from('attacker payload')]);
    expect(() =>
      verifyPackageBinarySet(
        'linux',
        packageSet(['appimage', 'deb', 'rpm', 'updater'], forged),
        expected(reviewed, 'x86_64-unknown-linux-gnu'),
      ),
    ).toThrow(/reviewed handoff|exact reviewed executable|payload identity/i);
  });

  test.each([
    ['ELF', 'linux', 'x86_64-unknown-linux-gnu', elf()],
    ['PE', 'windows', 'x86_64-pc-windows-msvc', pe()],
    ['Mach-O', 'macos', 'universal-apple-darwin', universalMachO()],
  ] as const)(
    'rejects a valid-looking arbitrary %s with a copied source marker',
    (_format, platform, target, reviewed) => {
      const arbitrary = Buffer.concat([reviewed, Buffer.from('different executable')]);
      const kinds =
        platform === 'linux'
          ? ['appimage', 'deb', 'rpm', 'updater']
          : platform === 'windows'
            ? ['nsis', 'updater']
            : ['dmg', 'updater'];
      expect(() =>
        verifyPackageBinarySet(
          platform,
          packageSet(kinds, arbitrary),
          expected(reviewed, target),
        ),
      ).toThrow(
        /reviewed handoff|exact reviewed executable|payload identity|unreviewed trailing bytes/i,
      );
    },
  );

  test('rejects four independently valid but different Linux package binaries', () => {
    const reviewed = elf();
    const kinds = ['appimage', 'deb', 'rpm', 'updater'];
    expect(() =>
      verifyPackageBinarySet(
        'linux',
        kinds.map((kind, index) => ({
          kind,
          binary: Buffer.concat([elf(), Buffer.from([index + 1])]),
          artifact: Buffer.from(`artifact-${kind}`),
          signatureAuthenticated: true,
          signatureIdentity: null,
          entrypoint: {
            contract:
              kind === 'appimage' || kind === 'updater'
                ? 'linux-apprun-target'
                : 'linux-package-install-target',
            path: 'usr/bin/mdreader-app',
          },
        })),
        expected(reviewed, 'x86_64-unknown-linux-gnu'),
      ),
    ).toThrow(/reviewed handoff|exact reviewed executable|payload identity/i);
  });

  test('rejects wrong handoff digest, source commit, and release version in final evidence', () => {
    const reviewed = elf();
    const packages = packageSet(
      ['appimage', 'deb', 'rpm', 'updater'],
      reviewed,
    );
    const evidence = verifyPackageBinarySet(
      'linux',
      packages,
      expected(reviewed, 'x86_64-unknown-linux-gnu'),
    );
    const artifacts = packages.map(({ kind, artifact }) => ({ kind, artifact }));

    expect(() =>
      verifyFinalPackageEvidence(evidence, artifacts, {
        ...expected(reviewed, 'x86_64-unknown-linux-gnu'),
        unsignedHandoffSha256: 'b'.repeat(64),
        platform: 'linux',
      }),
    ).toThrow(/handoff digest/i);
    expect(() =>
      verifyFinalPackageEvidence(evidence, artifacts, {
        ...expected(reviewed, 'x86_64-unknown-linux-gnu'),
        sourceCommit: 'f'.repeat(40),
        platform: 'linux',
      }),
    ).toThrow(/source commit/i);
    expect(() =>
      verifyFinalPackageEvidence(evidence, artifacts, {
        ...expected(reviewed, 'x86_64-unknown-linux-gnu'),
        expectedVersion: '1.0.1',
        platform: 'linux',
      }),
    ).toThrow(/release version/i);
  });

  test('rejects package substitution before staging consumes final evidence', () => {
    const reviewed = elf();
    const packages = packageSet(
      ['appimage', 'deb', 'rpm', 'updater'],
      reviewed,
    );
    const evidence = verifyPackageBinarySet(
      'linux',
      packages,
      expected(reviewed, 'x86_64-unknown-linux-gnu'),
    );
    const artifacts = packages.map(({ kind, artifact }) => ({
      kind,
      artifact:
        kind === 'rpm'
          ? Buffer.from('substituted-rpm-package')
          : artifact,
    }));
    expect(() =>
      verifyFinalPackageEvidence(evidence, artifacts, {
        ...expected(reviewed, 'x86_64-unknown-linux-gnu'),
        platform: 'linux',
      }),
    ).toThrow(/rpm.*package substitution|package substitution.*rpm/i);
  });

  test('rejects a reviewed executable that is merely ballast beside another package entrypoint', () => {
    const reviewed = elf();
    const packages = packageSet(
      ['appimage', 'deb', 'rpm', 'updater'],
      reviewed,
    );
    packages[0]!.entrypoint = {
      contract: 'linux-apprun-target',
      path: 'usr/bin/not-mdreader',
    };

    expect(() =>
      verifyPackageBinarySet(
        'linux',
        packages,
        expected(reviewed, 'x86_64-unknown-linux-gnu'),
      ),
    ).toThrow(/appimage.*entrypoint|entrypoint.*appimage/i);
  });

  test.each([
    [
      'macos',
      'universal-apple-darwin',
      ['dmg', 'updater'],
      universalMachO(),
      'AAAAAAAAAA',
      /publisher identity|team/i,
    ],
    [
      'windows',
      'x86_64-pc-windows-msvc',
      ['nsis', 'updater'],
      pe(),
      'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF',
      /publisher identity|thumbprint/i,
    ],
  ] as const)(
    'rejects a validly signed %s package from an unreviewed publisher',
    (platform, target, kinds, binary, unreviewedIdentity, error) => {
      const packages = packageSet(kinds, binary);
      packages[0]!.signatureIdentity = unreviewedIdentity;
      expect(() =>
        verifyPackageBinarySet(
          platform,
          packages,
          expected(binary, target),
        ),
      ).toThrow(error);
    },
  );
});
