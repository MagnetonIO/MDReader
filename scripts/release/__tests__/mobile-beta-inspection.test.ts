import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import {
  mkdtemp,
  mkdir,
  readFile,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, test } from 'vitest';

import {
  createMobileBetaInspectionToolRunner,
  inspectAndroidBetaArtifact,
  inspectIosBetaArtifact,
  type MobileBetaToolRunner,
} from '../mobile-beta-inspection';
import { retainMobileBetaArtifact } from '../mobile-beta-manifest';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const IOS_CERTIFICATE = Buffer.from('reviewed apple certificate');
const IOS_CERTIFICATE_SHA256 =
  '523f39bd77e2d7713ff07c041096132dbfe6d84cdfbb3a2e718579db84983891';
const IOS_PROFILE_SHA256 =
  '5701d5d8a5f1e3af1dab9a177ebfc7256267eae1e0d29ef5c58213cc67f6a903';
const ANDROID_CERTIFICATE_SHA256 = 'ab'.repeat(32);

function keytoolSignerOutput(
  certificateSha256 = ANDROID_CERTIFICATE_SHA256,
): string {
  return [
    'Signer #1:',
    '',
    'Certificate #1:',
    'Owner: CN=MDReader Test Upload',
    'Issuer: CN=MDReader Test Upload',
    'Certificate fingerprints:',
    `\t SHA256: ${certificateSha256.match(/../g)!.join(':')}`,
  ].join('\n');
}

function releaseIdentity(): Buffer {
  return Buffer.from(
    [
      'native-prefix',
      'MDREADER_RELEASE_IDENTITY_V1\0',
      'productName=MDReader\0',
      'bundleIdentifier=dev.mdreader.app\0',
      `sourceCommit=${SOURCE_COMMIT}\0`,
      'native-suffix',
    ].join(''),
  );
}

async function zipFixture(
  name: string,
  entries: Record<string, string | Buffer>,
): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-inspection-'));
  const staging = join(root, 'staging');
  for (const [path, body] of Object.entries(entries)) {
    const destination = join(staging, path);
    await mkdir(join(destination, '..'), { recursive: true });
    await writeFile(destination, body);
  }
  const artifact = join(root, name);
  execFileSync('zip', ['-q', '-r', artifact, '.'], { cwd: staging });
  return artifact;
}

async function renameEqualLengthZipEntry(
  archive: string,
  before: string,
  after: string,
): Promise<void> {
  const source = Buffer.from(before);
  const destination = Buffer.from(after);
  if (source.byteLength !== destination.byteLength) {
    throw new Error('test ZIP entry replacements must preserve byte length');
  }
  const body = await readFile(archive);
  let offset = 0;
  let replacements = 0;
  while ((offset = body.indexOf(source, offset)) >= 0) {
    destination.copy(body, offset);
    offset += destination.byteLength;
    replacements += 1;
  }
  if (replacements < 2) {
    throw new Error(`test ZIP did not contain ${before} in local and central headers`);
  }
  await writeFile(archive, body);
}

function delegate(command: string, args: readonly string[]) {
  return execFileSync(command, [...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

async function signedAndroidFixture(
  additionalSignedEntries: Record<string, string | Buffer> = {},
  entryRenames: ReadonlyArray<readonly [string, string]> = [],
): Promise<string> {
  return (
    await signedAndroidFixtureWithCertificate(
      additionalSignedEntries,
      entryRenames,
    )
  ).aab;
}

async function signedAndroidFixtureWithCertificate(
  additionalSignedEntries: Record<string, string | Buffer> = {},
  entryRenames: ReadonlyArray<readonly [string, string]> = [],
): Promise<{ aab: string; certificateSha256: string }> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-jdk-signer-'));
  const aab = await zipFixture('MDReader.aab', {
    'base/manifest/AndroidManifest.xml': 'binary manifest',
    'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
    ...additionalSignedEntries,
  });
  for (const [before, after] of entryRenames) {
    await renameEqualLengthZipEntry(aab, before, after);
  }
  const keyStore = join(root, 'upload.p12');
  execFileSync('keytool', [
    '-genkeypair',
    '-alias',
    'upload',
    '-keyalg',
    'RSA',
    '-keysize',
    '2048',
    '-validity',
    '3650',
    '-dname',
    'CN=MDReader Test Upload',
    '-storetype',
    'PKCS12',
    '-keystore',
    keyStore,
    '-storepass',
    'changeit',
    '-keypass',
    'changeit',
  ]);
  const certificateSha256 = createHash('sha256')
    .update(
      execFileSync('keytool', [
        '-exportcert',
        '-alias',
        'upload',
        '-keystore',
        keyStore,
        '-storepass',
        'changeit',
      ]),
    )
    .digest('hex');
  execFileSync('jarsigner', [
    '-keystore',
    keyStore,
    '-storetype',
    'PKCS12',
    '-storepass',
    'changeit',
    '-keypass',
    'changeit',
    aab,
    'upload',
  ]);
  return { aab, certificateSha256 };
}

async function addAndroidSigner(aab: string): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-jdk-second-signer-'));
  const keyStore = join(root, 'alternate.p12');
  execFileSync('keytool', [
    '-genkeypair',
    '-alias',
    'alternate',
    '-keyalg',
    'RSA',
    '-keysize',
    '2048',
    '-validity',
    '3650',
    '-dname',
    'CN=MDReader Alternate Upload',
    '-storetype',
    'PKCS12',
    '-keystore',
    keyStore,
    '-storepass',
    'changeit',
    '-keypass',
    'changeit',
  ]);
  const certificateSha256 = createHash('sha256')
    .update(
      execFileSync('keytool', [
        '-exportcert',
        '-alias',
        'alternate',
        '-keystore',
        keyStore,
        '-storepass',
        'changeit',
      ]),
    )
    .digest('hex');
  execFileSync('jarsigner', [
    '-keystore',
    keyStore,
    '-storetype',
    'PKCS12',
    '-storepass',
    'changeit',
    '-keypass',
    'changeit',
    aab,
    'alternate',
  ]);
  return certificateSha256;
}

async function addUnsignedAndroidEntries(
  aab: string,
  entries: Record<string, string | Buffer>,
): Promise<void> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-jdk-unsigned-'));
  for (const [path, body] of Object.entries(entries)) {
    const destination = join(root, path);
    await mkdir(join(destination, '..'), { recursive: true });
    await writeFile(destination, body);
  }
  execFileSync('zip', ['-q', '-r', aab, '.'], { cwd: root });
}

function androidRunnerWithRealJarSignature(): MobileBetaToolRunner {
  return (command, args) => {
    if (
      command === 'unzip' ||
      command === 'zipinfo' ||
      command === 'jar' ||
      command === 'jarsigner'
    ) {
      return delegate(command, args);
    }
    if (command === 'java' && args.includes('validate')) return '';
    if (command === 'java' && args.includes('--xpath=/manifest/@package')) {
      return 'dev.mdreader.app\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionName')
    ) {
      return '1.0.0\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionCode')
    ) {
      return '41\n';
    }
    if (command === 'keytool') {
      return keytoolSignerOutput();
    }
    throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
  };
}

function androidRunnerWithRealJarAuthority(): MobileBetaToolRunner {
  return (command, args) => {
    if (
      command === 'unzip' ||
      command === 'zipinfo' ||
      command === 'jar' ||
      command === 'jarsigner' ||
      command === 'keytool'
    ) {
      return delegate(command, args);
    }
    if (command === 'java' && args.includes('validate')) return '';
    if (command === 'java' && args.includes('--xpath=/manifest/@package')) {
      return 'dev.mdreader.app\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionName')
    ) {
      return '1.0.0\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionCode')
    ) {
      return '41\n';
    }
    throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
  };
}

function androidRunnerWithJarInventory(output: string): MobileBetaToolRunner {
  return (command, args) => {
    if (command === 'unzip') return delegate(command, args);
    if (command === 'zipinfo') return delegate(command, args);
    if (command === 'jar') return delegate(command, args);
    if (command === 'java' && args.includes('validate')) return '';
    if (command === 'java' && args.includes('--xpath=/manifest/@package')) {
      return 'dev.mdreader.app\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionName')
    ) {
      return '1.0.0\n';
    }
    if (
      command === 'java' &&
      args.includes('--xpath=/manifest/@android:versionCode')
    ) {
      return '41\n';
    }
    if (command === 'jarsigner') return output;
    if (command === 'keytool') {
      return keytoolSignerOutput();
    }
    throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
  };
}

function iosRunner(
  profileOverrides: Record<string, unknown> = {},
  signedEntitlementOverrides: Record<string, unknown> = {},
): MobileBetaToolRunner {
  return (command, args) => {
    if (command === 'unzip') return delegate(command, args);
    if (command === 'zipinfo') return delegate(command, args);
    if (command === 'plutil') {
      const path = args.at(-1)!;
      if (path.endsWith('Info.plist')) {
        return JSON.stringify({
          CFBundleIdentifier: 'dev.mdreader.app',
          CFBundleExecutable: 'MDReader',
          CFBundleShortVersionString: '1.0.0',
          CFBundleVersion: '41',
        });
      }
      if (path.endsWith('profile.plist')) {
        return JSON.stringify({
          UUID: '11111111-2222-3333-4444-555555555555',
          Name: 'MDReader App Store',
          ExpirationDate: '2027-08-01T00:00:00.000Z',
          TeamIdentifier: ['7727LYTG96'],
          Entitlements: {
            'application-identifier': '7727LYTG96.dev.mdreader.app',
            'com.apple.developer.team-identifier': '7727LYTG96',
            'get-task-allow': false,
            'beta-reports-active': true,
            'keychain-access-groups': ['7727LYTG96.*'],
          },
          ...profileOverrides,
        });
      }
      if (path.endsWith('entitlements.plist')) {
        return JSON.stringify({
          'application-identifier': '7727LYTG96.dev.mdreader.app',
          'com.apple.developer.team-identifier': '7727LYTG96',
          'get-task-allow': false,
          ...signedEntitlementOverrides,
        });
      }
      throw new Error(`unexpected plutil path ${path}`);
    }
    if (command === 'security') {
      return `<?xml version="1.0"?><plist><dict><key>placeholder</key><true/></dict></plist>`;
    }
    if (command === 'lipo') return 'arm64\n';
    if (command === 'codesign' && args.includes('--verify')) return '';
    if (command === 'codesign' && args.includes('--entitlements')) {
      return `<?xml version="1.0"?><plist><dict><key>placeholder</key><true/></dict></plist>`;
    }
    if (command === 'codesign' && args.some((arg) => arg.startsWith('--extract-certificates='))) {
      const prefix = args
        .find((arg) => arg.startsWith('--extract-certificates='))!
        .slice('--extract-certificates='.length);
      writeFileSync(`${prefix}0`, IOS_CERTIFICATE);
      return '';
    }
    if (command === 'codesign' && args.includes('--verbose=4')) {
      return [
        'Authority=Apple Distribution: Magneton Labs, LLC (7727LYTG96)',
        'TeamIdentifier=7727LYTG96',
      ].join('\n');
    }
    throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
  };
}

describe('signed mobile artifact inspection', () => {
  test('derives the iOS package, build, device architecture, source identity, and signer from the IPA', async () => {
    const ipa = await zipFixture('MDReader.ipa', {
      'Payload/MDReader.app/Info.plist': '<plist/>',
      'Payload/MDReader.app/MDReader': releaseIdentity(),
      'Payload/MDReader.app/embedded.mobileprovision': 'signed profile',
    });

    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        iosRunner(),
      ),
    ).resolves.toMatchObject({
      platform: 'ios',
      channel: 'testflight',
      packageId: 'dev.mdreader.app',
      architectures: ['arm64'],
      sourceCommit: SOURCE_COMMIT,
      signingIdentity: {
        kind: 'apple-distribution',
        teamId: '7727LYTG96',
        certificateSha256: IOS_CERTIFICATE_SHA256,
      },
      iosAuthority: {
        provisioningProfileUuid: '11111111-2222-3333-4444-555555555555',
        provisioningProfileSha256: IOS_PROFILE_SHA256,
        profileEntitlementsSha256:
          '997964ec46b6b5043049cc6925dfaf59e7cfa514d485d71ead582a420aca5f2e',
        signedEntitlementsSha256:
          'e869cce0e4f2d0da1e8a04210f512691fc343ebffaaad4ebfe6cb704aa90c42b',
      },
    });
  });

  test('rejects an IPA with a traversal entry before extraction', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-traversal-'));
    const staging = join(root, 'staging');
    await mkdir(staging);
    const payload = join(root, 'outside');
    await writeFile(payload, 'malicious');
    const ipa = join(root, 'MDReader.ipa');
    execFileSync('zip', ['-q', ipa, '../outside'], { cwd: staging });

    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        iosRunner(),
      ),
    ).rejects.toThrow(/unsafe ZIP entry/);
  });

  test('rejects an archive child beneath a symlink before extraction', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-symlink-parent-'));
    const staging = join(root, 'staging');
    await mkdir(join(staging, 'Payload/MDReader.app/real'), {
      recursive: true,
    });
    await writeFile(
      join(staging, 'Payload/MDReader.app/Info.plist'),
      '<plist/>',
    );
    await symlink(
      '../../../outside',
      join(staging, 'Payload/MDReader.app/escape'),
    );
    await writeFile(
      join(staging, 'Payload/MDReader.app/real/owned'),
      'must never extract',
    );
    const ipa = join(root, 'MDReader.ipa');
    execFileSync('zip', ['-q', '-y', '-r', ipa, '.'], { cwd: staging });
    await renameEqualLengthZipEntry(
      ipa,
      'Payload/MDReader.app/real/owned',
      'Payload/MDReader.app/link/owned',
    );

    let extractions = 0;
    const base = iosRunner();
    const runner: MobileBetaToolRunner = (command, args) => {
      if (command === 'unzip' && args.includes('-d')) {
        extractions += 1;
        throw new Error('unsafe package reached extraction');
      }
      return base(command, args);
    };

    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        runner,
      ),
    ).rejects.toThrow(/archive symlink/);
    expect(extractions).toBe(0);
  });

  test('rejects case-folding ZIP path collisions before extraction', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-case-collision-'));
    const staging = join(root, 'staging');
    await mkdir(join(staging, 'Payload/MDReader.app'), { recursive: true });
    await writeFile(
      join(staging, 'Payload/MDReader.app/Info.plist'),
      '<plist/>',
    );
    await writeFile(join(staging, 'aaaaaa'), 'one');
    await writeFile(join(staging, 'bbbbbb'), 'two');
    const ipa = join(root, 'MDReader.ipa');
    execFileSync(
      'zip',
      [
        '-q',
        ipa,
        'Payload/MDReader.app/Info.plist',
        'aaaaaa',
        'bbbbbb',
      ],
      { cwd: staging },
    );
    await renameEqualLengthZipEntry(
      ipa,
      'aaaaaa',
      'Readme',
    );
    await renameEqualLengthZipEntry(
      ipa,
      'bbbbbb',
      'README',
    );
    let extractions = 0;
    const base = iosRunner();
    const runner: MobileBetaToolRunner = (command, args) => {
      if (command === 'unzip' && args.includes('-d')) {
        extractions += 1;
        throw new Error('unsafe package reached extraction');
      }
      return base(command, args);
    };

    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        runner,
      ),
    ).rejects.toThrow(/collision/);
    expect(extractions).toBe(0);
  });

  test.each([
    [
      'development or ad hoc device profile',
      { ProvisionedDevices: ['00000000-0000-0000-0000-000000000001'] },
    ],
    ['enterprise profile', { ProvisionsAllDevices: true }],
    ['expired profile', { ExpirationDate: '2026-01-01T00:00:00.000Z' }],
    [
      'profile without TestFlight entitlement',
      {
        Entitlements: {
          'application-identifier': '7727LYTG96.dev.mdreader.app',
          'get-task-allow': false,
          'beta-reports-active': false,
        },
      },
    ],
  ])('rejects an iOS %s', async (_name, profileOverrides) => {
    const ipa = await zipFixture('MDReader.ipa', {
      'Payload/MDReader.app/Info.plist': '<plist/>',
      'Payload/MDReader.app/MDReader': releaseIdentity(),
      'Payload/MDReader.app/embedded.mobileprovision': 'signed profile',
    });
    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        iosRunner(profileOverrides),
      ),
    ).rejects.toThrow(/App Store|TestFlight|expired/);
  });

  test.each([
    [
      'provisioning profile entitlement',
      {
        Entitlements: {
          'application-identifier': '7727LYTG96.dev.mdreader.app',
          'com.apple.developer.team-identifier': '7727LYTG96',
          'get-task-allow': false,
          'beta-reports-active': true,
          'keychain-access-groups': ['7727LYTG96.*'],
          'aps-environment': 'production',
        },
      },
      {},
    ],
    [
      'signed entitlement',
      {},
      { 'com.apple.developer.networking.wifi-info': true },
    ],
  ])('rejects an unexpected iOS %s', async (_name, profile, signed) => {
    const ipa = await zipFixture('MDReader.ipa', {
      'Payload/MDReader.app/Info.plist': '<plist/>',
      'Payload/MDReader.app/MDReader': releaseIdentity(),
      'Payload/MDReader.app/embedded.mobileprovision': 'signed profile',
    });
    await expect(
      inspectIosBetaArtifact(
        ipa,
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          teamId: '7727LYTG96',
          certificateSha256: IOS_CERTIFICATE_SHA256,
          provisioningProfileSha256: IOS_PROFILE_SHA256,
        },
        iosRunner(profile, signed),
      ),
    ).rejects.toThrow(/unexpected.*entitlement/i);
  });

  test('derives the Android package, build, ABI, source identity, and signer from the signed AAB', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    const runner: MobileBetaToolRunner = (command, args) => {
      if (command === 'unzip') return delegate(command, args);
      if (command === 'zipinfo') return delegate(command, args);
      if (command === 'jar') return delegate(command, args);
      if (command === 'java' && args.includes('validate')) return '';
      if (command === 'java' && args.includes('--xpath=/manifest/@package')) {
        return 'dev.mdreader.app\n';
      }
      if (
        command === 'java' &&
        args.includes('--xpath=/manifest/@android:versionName')
      ) {
        return '1.0.0\n';
      }
      if (
        command === 'java' &&
        args.includes('--xpath=/manifest/@android:versionCode')
      ) {
        return '41\n';
      }
      if (command === 'jarsigner') {
        return [
          'sm 15 Tue Jul 28 04:43:25 UTC 2026 base/manifest/AndroidManifest.xml',
          'sm 164 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
          'jar verified.',
        ].join('\n');
      }
      if (command === 'keytool') {
        return keytoolSignerOutput();
      }
      throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
    };

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        runner,
      ),
    ).resolves.toMatchObject({
      platform: 'android',
      channel: 'google-play-closed',
      packageId: 'dev.mdreader.app',
      architectures: ['arm64-v8a'],
      sourceCommit: SOURCE_COMMIT,
      signingIdentity: {
        kind: 'google-play-upload',
        certificateSha256: ANDROID_CERTIFICATE_SHA256,
      },
    });
  });

  test('rejects an AAB with an unsigned entry even when non-strict jarsigner reports it verified', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-unsigned-entry-'));
    const aab = await signedAndroidFixture();
    await writeFile(join(root, 'unsigned.txt'), 'not covered by the signature');
    execFileSync('zip', ['-q', aab, 'unsigned.txt'], { cwd: root });
    expect(() =>
      execFileSync('jarsigner', ['-verify', '-verbose', '-certs', aab]),
    ).not.toThrow();

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithRealJarSignature(),
      ),
    ).rejects.toThrow(/unsigned|signature coverage/i);
  });

  test('rejects unsigned foo when a different signed entry is named bar foo', async () => {
    const aab = await signedAndroidFixture({
      'bar foo': 'signed suffix decoy',
    });
    await addUnsignedAndroidEntries(aab, {
      foo: 'unsigned payload',
    });

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithRealJarSignature(),
      ),
    ).rejects.toThrow(/signature coverage/i);
  });

  test('maps real jarsigner rows to exact whitespace, Unicode, and nested ZIP identities', async () => {
    const aab = await signedAndroidFixture({
      ' leading.txt': 'leading whitespace',
      'repeated  whitespace.txt': 'repeated whitespace',
      'trailing.txt ': 'trailing whitespace',
      'unicode/cafxx.txt': 'precomposed Unicode',
      'unicode/cafxxx-other.txt': 'decomposed Unicode',
      'nested/suffix.txt': 'nested suffix',
      'suffix.txt': 'root suffix',
    }, [
      ['unicode/cafxx.txt', 'unicode/café.txt'],
      ['unicode/cafxxx-other.txt', 'unicode/cafe\u0301-other.txt'],
    ]);

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithRealJarSignature(),
      ),
    ).resolves.toMatchObject({
      platform: 'android',
      packageId: 'dev.mdreader.app',
      architectures: ['arm64-v8a'],
    });
  });

  test('rejects duplicate jarsigner inventory records for one ZIP entry', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    const inventory = [
      'sm 15 Tue Jul 28 04:43:25 UTC 2026 base/manifest/AndroidManifest.xml',
      'sm 164 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
      'sm 164 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
      'jar verified.',
    ].join('\n');

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithJarInventory(inventory),
      ),
    ).rejects.toThrow(/duplicate.*inventory/i);
  });

  test('rejects a jarsigner inventory record without an exact ZIP entry', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    const inventory = [
      'sm 15 Tue Jul 28 04:43:25 UTC 2026 base/manifest/AndroidManifest.xml',
      'sm 164 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
      'sm 10 Tue Jul 28 04:43:25 UTC 2026 not/in/the/archive',
      'jar verified.',
    ].join('\n');

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithJarInventory(inventory),
      ),
    ).rejects.toThrow(/inventory.*ZIP entry/i);
  });

  test('rejects a jarsigner inventory record with the wrong entry size', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    const inventory = [
      'sm 999 Tue Jul 28 04:43:25 UTC 2026 base/manifest/AndroidManifest.xml',
      'sm 164 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
      'jar verified.',
    ].join('\n');

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithJarInventory(inventory),
      ),
    ).rejects.toThrow(/inventory.*size/i);
  });

  test('rejects unsigned AAB bundle metadata rather than exempting all META-INF content', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-unsigned-meta-'));
    const metadata = join(
      root,
      'META-INF',
      'BUNDLE-METADATA',
      'dev.mdreader',
      'metadata.pb',
    );
    await mkdir(join(metadata, '..'), { recursive: true });
    await writeFile(metadata, 'unsigned bundle metadata');
    const aab = await signedAndroidFixture();
    execFileSync('zip', ['-q', aab, 'META-INF/BUNDLE-METADATA/dev.mdreader/metadata.pb'], {
      cwd: root,
    });

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithRealJarSignature(),
      ),
    ).rejects.toThrow(/signature coverage/i);
  });

  test('rejects a signed AAB whose native code is not bound to the requested source', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': 'no release identity',
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    const runner: MobileBetaToolRunner = (command, args) => {
      if (command === 'unzip') return delegate(command, args);
      if (command === 'zipinfo') return delegate(command, args);
      if (command === 'jar') return delegate(command, args);
      if (command === 'java' && args.includes('validate')) return '';
      if (command === 'java' && args.some((arg) => arg.includes('@package'))) {
        return 'dev.mdreader.app\n';
      }
      if (command === 'java' && args.some((arg) => arg.includes('versionName'))) {
        return '1.0.0\n';
      }
      if (command === 'java' && args.some((arg) => arg.includes('versionCode'))) {
        return '41\n';
      }
      if (command === 'jarsigner') {
        return [
          'sm 15 Tue Jul 28 04:43:25 UTC 2026 base/manifest/AndroidManifest.xml',
          'sm 19 Tue Jul 28 04:43:25 UTC 2026 base/lib/arm64-v8a/libmdreader_app_lib.so',
          'jar verified.',
        ].join('\n');
      }
      if (command === 'keytool') return keytoolSignerOutput();
      throw new Error(`unexpected tool call ${command} ${args.join(' ')}`);
    };

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        runner,
      ),
    ).rejects.toThrow(/release identity/);
  });

  test('rejects a marker-bearing sibling when the Tauri application library lacks the source identity', async () => {
    const aab = await signedAndroidFixture({
      'base/lib/arm64-v8a/libmdreader_app_lib.so': 'no release identity',
      'base/lib/arm64-v8a/libidentity_decoy.so': releaseIdentity(),
    });

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        androidRunnerWithRealJarSignature(),
      ),
    ).rejects.toThrow(/libmdreader_app_lib\.so.*release identity/i);
  });

  test('rejects a real App Bundle with an additional JAR signer', async () => {
    const fixture = await signedAndroidFixtureWithCertificate();
    const latestSignerSha256 = await addAndroidSigner(fixture.aab);

    await expect(
      inspectAndroidBetaArtifact(
        fixture.aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: latestSignerSha256,
        },
        androidRunnerWithRealJarAuthority(),
      ),
    ).rejects.toThrow(/exactly one.*signer/i);
  });

  test('accepts a real App Bundle whose only JAR signer is the reviewed certificate', async () => {
    const fixture = await signedAndroidFixtureWithCertificate();

    await expect(
      inspectAndroidBetaArtifact(
        fixture.aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: fixture.certificateSha256,
        },
        androidRunnerWithRealJarAuthority(),
      ),
    ).resolves.toMatchObject({
      platform: 'android',
      signingIdentity: {
        kind: 'google-play-upload',
        certificateSha256: fixture.certificateSha256,
      },
    });
  });

  test('rejects a ZIP bomb before extraction or native-entry reads', async () => {
    const aab = await zipFixture('MDReader.aab', {
      'base/manifest/AndroidManifest.xml': 'binary manifest',
      'base/lib/arm64-v8a/libmdreader_app_lib.so': releaseIdentity(),
      'META-INF/MDR.SF': 'signature metadata',
      'META-INF/MDR.RSA': 'signature block',
    });
    let unzipReads = 0;
    const runner: MobileBetaToolRunner = (command, args) => {
      if (command === 'unzip' && args[0] === '-Z1') {
        return delegate(command, args);
      }
      if (command === 'unzip') {
        unzipReads += 1;
        return delegate(command, args);
      }
      if (command === 'zipinfo') {
        return delegate(command, args)
          .split(/\r?\n/)
          .map((line) =>
            line.endsWith(
              'base/lib/arm64-v8a/libmdreader_app_lib.so',
            )
              ? line.replace(
                  /\s+\d+\s+bx\s+\d+\s+/,
                  ' 3221225472 bx 32 ',
                )
              : line,
          )
          .join('\n');
      }
      throw new Error(`unsafe package reached ${command}`);
    };

    await expect(
      inspectAndroidBetaArtifact(
        aab,
        '/reviewed/bundletool.jar',
        {
          sourceCommit: SOURCE_COMMIT,
          version: '1.0.0',
          buildNumber: '41',
          certificateSha256: ANDROID_CERTIFICATE_SHA256,
        },
        runner,
      ),
    ).rejects.toThrow(/ZIP.*limit/);
    expect(unzipReads).toBe(0);
  });

  test('fails closed when a package inspection subprocess exceeds its deadline', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-mobile-tool-timeout-'));
    const artifact = join(root, 'MDReader.ipa');
    const tool = join(root, 'unzip');
    await writeFile(artifact, 'not reached');
    await writeFile(
      tool,
      '#!/usr/bin/env node\nsetTimeout(() => undefined, 1000);\n',
      { mode: 0o755 },
    );
    const originalPath = process.env.PATH;
    process.env.PATH = `${root}:${originalPath ?? ''}`;
    const started = Date.now();
    try {
      await expect(
        inspectIosBetaArtifact(
          artifact,
          {
            sourceCommit: SOURCE_COMMIT,
            version: '1.0.0',
            buildNumber: '41',
            teamId: '7727LYTG96',
            certificateSha256: IOS_CERTIFICATE_SHA256,
            provisioningProfileSha256: IOS_PROFILE_SHA256,
          },
          undefined,
          { toolTimeoutMs: 50 },
        ),
      ).rejects.toThrow(/timed out/i);
      expect(Date.now() - started).toBeLessThan(500);
    } finally {
      process.env.PATH = originalPath;
    }
  });

  test('runs archive tools against the unlinked descriptor after retained-path rewrite', async () => {
    const artifact = await zipFixture('MDReader.ipa', {
      'Payload/MDReader.app/original': 'reviewed',
    });
    const replacement = await zipFixture('attacker.ipa', {
      'Payload/MDReader.app/attacker': 'different',
    });
    const snapshot = join(artifact, '..', 'retained.ipa');
    const retained = await retainMobileBetaArtifact(artifact, snapshot);
    await writeFile(snapshot, await readFile(replacement));
    try {
      const run = createMobileBetaInspectionToolRunner(
        5_000,
        retained.openedArtifact.handle.fd,
      );
      const entries = run('unzip', [
        '-Z1',
        `/dev/fd/${retained.openedArtifact.handle.fd}`,
      ]).split(/\r?\n/);
      expect(entries).toContain('Payload/MDReader.app/original');
      expect(entries).not.toContain('Payload/MDReader.app/attacker');
      const jdkEntries = run('jar', [
        '-J-Dfile.encoding=UTF-8',
        '--list',
        '--file',
        `/dev/fd/${retained.openedArtifact.handle.fd}`,
      ]).split(/\r?\n/);
      expect(jdkEntries).toContain('Payload/MDReader.app/original');
      expect(jdkEntries).not.toContain('Payload/MDReader.app/attacker');
    } finally {
      await retained.openedArtifact.handle.close();
    }
  });
});
