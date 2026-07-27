import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import {
  appendFile,
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import { test } from 'node:test';

import {
  androidCapabilitiesFor,
  iosCapabilitiesFor,
  validateAndroidApk,
} from './capabilities.ts';
import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const CENTRAL_SIGNATURE = 0x02014b50;
const EOCD_SIGNATURE = 0x06054b50;

function buildManifest(sourceCommit = SOURCE_COMMIT): string {
  return `MDREADER_E2E_BUILD_MANIFEST:${JSON.stringify({
      schema: 1,
      source_commit: sourceCommit,
      bundle_identifier: E2E_BUNDLE_IDENTIFIER,
      profile: 'e2e-build',
    })}:MDREADER_E2E_BUILD_MANIFEST_END`;
}

function compiledAndroidManifest(packageName = E2E_BUNDLE_IDENTIFIER): Buffer {
  const strings = ['manifest', 'package', packageName];
  const encodedStrings = strings.map((value) => {
    const bytes = Buffer.from(value, 'utf8');
    if (value.length >= 0x80 || bytes.length >= 0x80) {
      throw new Error('test manifest strings must fit one-byte lengths');
    }
    return Buffer.concat([
      Buffer.from([value.length, bytes.length]),
      bytes,
      Buffer.from([0]),
    ]);
  });
  const stringOffsets = Buffer.alloc(strings.length * 4);
  let stringOffset = 0;
  for (const [index, value] of encodedStrings.entries()) {
    stringOffsets.writeUInt32LE(stringOffset, index * 4);
    stringOffset += value.length;
  }
  const stringDataLength = (stringOffset + 3) & ~3;
  const stringData = Buffer.alloc(stringDataLength);
  let cursor = 0;
  for (const value of encodedStrings) {
    value.copy(stringData, cursor);
    cursor += value.length;
  }
  const stringPool = Buffer.alloc(28);
  stringPool.writeUInt16LE(0x0001, 0);
  stringPool.writeUInt16LE(28, 2);
  stringPool.writeUInt32LE(28 + stringOffsets.length + stringData.length, 4);
  stringPool.writeUInt32LE(strings.length, 8);
  stringPool.writeUInt32LE(0, 12);
  stringPool.writeUInt32LE(0x00000100, 16);
  stringPool.writeUInt32LE(28 + stringOffsets.length, 20);
  stringPool.writeUInt32LE(0, 24);

  const startElement = Buffer.alloc(56, 0);
  startElement.writeUInt16LE(0x0102, 0);
  startElement.writeUInt16LE(16, 2);
  startElement.writeUInt32LE(startElement.length, 4);
  startElement.writeUInt32LE(1, 8);
  startElement.writeUInt32LE(0xffffffff, 12);
  startElement.writeUInt32LE(0xffffffff, 16);
  startElement.writeUInt32LE(0, 20);
  startElement.writeUInt16LE(20, 24);
  startElement.writeUInt16LE(20, 26);
  startElement.writeUInt16LE(1, 28);
  startElement.writeUInt32LE(0xffffffff, 36);
  startElement.writeUInt32LE(1, 40);
  startElement.writeUInt32LE(2, 44);
  startElement.writeUInt16LE(8, 48);
  startElement.writeUInt8(0, 50);
  startElement.writeUInt8(0x03, 51);
  startElement.writeUInt32LE(2, 52);

  const body = Buffer.concat([stringPool, stringOffsets, stringData, startElement]);
  const xml = Buffer.alloc(8);
  xml.writeUInt16LE(0x0003, 0);
  xml.writeUInt16LE(8, 2);
  xml.writeUInt32LE(xml.length + body.length, 4);
  return Buffer.concat([xml, body]);
}

const exec = promisify(execFile);

async function fakeAndroidArtifact(
  nativeLibrary: Buffer,
  extraNativeLibraries: ReadonlyArray<readonly [string, Buffer]> = [],
  extraFiles: ReadonlyArray<readonly [string, Buffer]> = [],
  manifestPackage = E2E_BUNDLE_IDENTIFIER,
): Promise<{
  apk: string;
  root: string;
  tools: string;
}> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-android-bound-'));
  const payload = join(root, 'payload');
  const tools = join(root, 'tools');
  const apk = join(root, 'bounded.apk');
  await mkdir(join(payload, 'lib', 'x86_64'), { recursive: true });
  await mkdir(tools);
  await writeFile(
    join(payload, 'AndroidManifest.xml'),
    compiledAndroidManifest(manifestPackage),
  );
  await writeFile(join(payload, 'classes.dex'), Buffer.from('dex\n035\0'));
  await writeFile(join(payload, 'lib', 'x86_64', 'libmdreader_app_lib.so'), nativeLibrary);
  for (const [name, bytes] of extraNativeLibraries) {
    await writeFile(join(payload, 'lib', 'x86_64', name), bytes);
  }
  for (const [name, bytes] of extraFiles) {
    const path = join(payload, name);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, bytes);
  }
  await exec('/usr/bin/zip', ['-qr', apk, '.'], { cwd: payload });
  for (const executable of ['apkanalyzer', 'unzip']) {
    const forbidden = join(tools, executable);
    await writeFile(forbidden, '#!/bin/sh\nexit 97\n');
    await chmod(forbidden, 0o755);
  }
  return { apk, root, tools };
}

interface ZipRecord {
  centralOffset: number;
  localOffset: number;
  name: string;
  nameLength: number;
  extraOffset: number;
  extraLength: number;
}

function zipRecords(bytes: Buffer): ZipRecord[] {
  const eocd = bytes.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (eocd < 0 || bytes.readUInt32LE(eocd) !== EOCD_SIGNATURE) {
    throw new Error('test APK has no EOCD');
  }
  const entries = bytes.readUInt16LE(eocd + 10);
  let offset = bytes.readUInt32LE(eocd + 16);
  const records: ZipRecord[] = [];
  for (let index = 0; index < entries; index += 1) {
    if (bytes.readUInt32LE(offset) !== CENTRAL_SIGNATURE) {
      throw new Error('test APK has an invalid central directory');
    }
    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const nameStart = offset + 46;
    records.push({
      centralOffset: offset,
      localOffset: bytes.readUInt32LE(offset + 42),
      name: bytes.subarray(nameStart, nameStart + nameLength).toString('utf8'),
      nameLength,
      extraOffset: nameStart + nameLength,
      extraLength,
    });
    offset += 46 + nameLength + extraLength + commentLength;
  }
  return records;
}

function zipRecord(bytes: Buffer, member: string): ZipRecord {
  const record = zipRecords(bytes).find((candidate) => candidate.name === member);
  if (!record) throw new Error(`missing ZIP member ${member}`);
  return record;
}

async function setDeclaredSize(
  apk: string,
  member: string,
  declaredSize: number,
): Promise<void> {
  const bytes = await readFile(apk);
  const signature = Buffer.from([0x50, 0x4b, 0x01, 0x02]);
  for (let offset = bytes.indexOf(signature); offset >= 0; offset = bytes.indexOf(signature, offset + 1)) {
    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const nameStart = offset + 46;
    const name = bytes.subarray(nameStart, nameStart + nameLength).toString('utf8');
    if (name === member) {
      bytes.writeUInt32LE(declaredSize, offset + 24);
      await writeFile(apk, bytes);
      return;
    }
    offset = nameStart + nameLength + extraLength + commentLength - 1;
  }
  throw new Error(`missing ZIP member ${member}`);
}

async function setCentralU32(
  apk: string,
  member: string,
  fieldOffset: number,
  value: number,
): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, member);
  bytes.writeUInt32LE(value, record.centralOffset + fieldOffset);
  await writeFile(apk, bytes);
}

async function setCentralU16(
  apk: string,
  member: string,
  fieldOffset: number,
  value: number,
): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, member);
  bytes.writeUInt16LE(value, record.centralOffset + fieldOffset);
  await writeFile(apk, bytes);
}

async function setZip64ExtraRecord(apk: string, member: string): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, member);
  if (record.extraLength < 4) throw new Error(`ZIP member ${member} has no mutable extra record`);
  bytes.writeUInt16LE(0x0001, record.extraOffset);
  await writeFile(apk, bytes);
}

async function insertZip64EndStructures(apk: string): Promise<void> {
  const bytes = await readFile(apk);
  const eocd = bytes.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (eocd < 0 || eocd + 22 !== bytes.length) {
    throw new Error('test APK must have one empty EOCD comment');
  }
  const zip64Eocd = Buffer.alloc(56);
  zip64Eocd.writeUInt32LE(0x06064b50, 0);
  zip64Eocd.writeBigUInt64LE(44n, 4);
  zip64Eocd.writeUInt16LE(45, 12);
  zip64Eocd.writeUInt16LE(45, 14);
  zip64Eocd.writeUInt32LE(0, 16);
  zip64Eocd.writeUInt32LE(0, 20);
  zip64Eocd.writeBigUInt64LE(BigInt(bytes.readUInt16LE(eocd + 8)), 24);
  zip64Eocd.writeBigUInt64LE(BigInt(bytes.readUInt16LE(eocd + 10)), 32);
  zip64Eocd.writeBigUInt64LE(BigInt(bytes.readUInt32LE(eocd + 12)), 40);
  zip64Eocd.writeBigUInt64LE(BigInt(bytes.readUInt32LE(eocd + 16)), 48);
  const locator = Buffer.alloc(20);
  locator.writeUInt32LE(0x07064b50, 0);
  locator.writeUInt32LE(0, 4);
  locator.writeBigUInt64LE(BigInt(eocd), 8);
  locator.writeUInt32LE(1, 16);
  const output = Buffer.concat([
    bytes.subarray(0, eocd),
    zip64Eocd,
    locator,
    bytes.subarray(eocd),
  ]);
  await writeFile(apk, output);
}

async function appendEocdComment(apk: string, comment: Buffer): Promise<void> {
  if (comment.length > 0xffff) throw new Error('test EOCD comment is too large');
  const bytes = await readFile(apk);
  const eocd = bytes.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (eocd < 0 || eocd + 22 !== bytes.length) {
    throw new Error('test APK must have one empty EOCD comment');
  }
  bytes.writeUInt16LE(comment.length, eocd + 20);
  await writeFile(apk, Buffer.concat([bytes, comment]));
}

async function renameZipMember(
  apk: string,
  from: string,
  to: string,
): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, from);
  const replacement = Buffer.from(to, 'utf8');
  if (replacement.length !== record.nameLength) {
    throw new Error('test ZIP member rename must preserve byte length');
  }
  replacement.copy(bytes, record.centralOffset + 46);
  if (bytes.readUInt32LE(record.localOffset) !== 0x04034b50) {
    throw new Error('test APK has an invalid local member header');
  }
  const localNameLength = bytes.readUInt16LE(record.localOffset + 26);
  if (localNameLength !== replacement.length) {
    throw new Error('test local and central member names differ in length');
  }
  replacement.copy(bytes, record.localOffset + 30);
  await writeFile(apk, bytes);
}

function plist(bundleIdentifier: string, executable: string): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${bundleIdentifier}</string>
<key>CFBundleExecutable</key><string>${executable}</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>`;
}

test('iOS capabilities reject a missing or incomplete simulator app', async () => {
  assert.throws(() => iosCapabilitiesFor(undefined), /MDREADER_IOS_APP/);

  const incomplete = join(tmpdir(), `mdreader-incomplete-${process.pid}.app`);
  await mkdir(incomplete, { recursive: true });
  assert.throws(() => iosCapabilitiesFor(incomplete), /Info\.plist/);
});

test('iOS capabilities reject malformed and production-identity bundles', async () => {
  const app = join(tmpdir(), `mdreader-production-${process.pid}.app`);
  await mkdir(app, { recursive: true });
  await writeFile(join(app, 'Info.plist'), 'not a plist');
  assert.throws(() => iosCapabilitiesFor(app), /property list/);

  await writeFile(join(app, 'Info.plist'), plist('dev.mdreader.app', 'MDReader'));
  await writeFile(join(app, 'MDReader'), 'binary');
  assert.throws(() => iosCapabilitiesFor(app), /dev\.mdreader\.app\.e2e/);
});

test('iOS capabilities reject a text executable behind a valid-looking plist', async () => {
  const app = join(tmpdir(), `mdreader-spoofed-${process.pid}.app`);
  await mkdir(app, { recursive: true });
  await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
  await writeFile(join(app, 'MDReader E2E'), 'not a Mach-O executable');
  await chmod(join(app, 'MDReader E2E'), 0o755);

  assert.throws(() => iosCapabilitiesFor(app), /Mach-O|Simulator/);
});

test(
  'iOS capabilities bind the exact e2e bundle to a real arm64 Simulator executable',
  { skip: process.platform !== 'darwin' },
  async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-ios-artifact-'));
    const app = join(root, 'MDReader E2E.app');
    const source = join(root, 'main.c');
    const executable = join(app, 'MDReader E2E');
    await mkdir(app, { recursive: true });
    await writeFile(source, 'int main(void) { return 0; }\n');
    await exec('/usr/bin/xcrun', [
      '--sdk',
      'iphonesimulator',
      'clang',
      '-target',
      'arm64-apple-ios14.0-simulator',
      source,
      '-o',
      executable,
    ]);
    await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
    await appendFile(executable, buildManifest());

    const capabilities = iosCapabilitiesFor(app, SOURCE_COMMIT);
    assert.equal(capabilities['appium:app'], app);
    assert.equal(capabilities['appium:bundleId'], E2E_BUNDLE_IDENTIFIER);
    await rm(root, { recursive: true });
  },
);

test('iOS capabilities reject an otherwise valid app built from a stale source commit', async () => {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-ios-stale-'));
  const app = join(root, 'MDReader E2E.app');
  const source = join(root, 'main.c');
  const executable = join(app, 'MDReader E2E');
  await mkdir(app, { recursive: true });
  await writeFile(source, 'int main(void) { return 0; }\n');
  await exec('/usr/bin/xcrun', [
    '--sdk',
    'iphonesimulator',
    'clang',
    '-target',
    'arm64-apple-ios14.0-simulator',
    source,
    '-o',
    executable,
  ]);
  await chmod(executable, 0o755);
  await writeFile(join(app, 'Info.plist'), plist(E2E_BUNDLE_IDENTIFIER, 'MDReader E2E'));
  await appendFile(executable, buildManifest('f'.repeat(40)));

  assert.throws(() => iosCapabilitiesFor(app, SOURCE_COMMIT), /source commit/i);
  await rm(root, { recursive: true });
});

test('Android capabilities require a real APK path', async () => {
  assert.throws(() => androidCapabilitiesFor(undefined), /MDREADER_ANDROID_APP/);

  const apk = join(tmpdir(), `mdreader-${process.pid}.apk`);
  await writeFile(apk, '');
  assert.throws(() => androidCapabilitiesFor(apk), /empty/);
  await writeFile(apk, 'test fixture');
  assert.throws(() => androidCapabilitiesFor(apk), /APK|ZIP/);
});

test('Android capabilities reject a ZIP-shaped package without a compiled manifest', async () => {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-android-spoof-'));
  const payload = join(root, 'payload');
  const apk = join(root, 'spoof.apk');
  await mkdir(payload);
  await writeFile(
    join(payload, 'AndroidManifest.xml'),
    `<manifest package="${E2E_BUNDLE_IDENTIFIER}"></manifest>`,
  );
  await exec('/usr/bin/zip', ['-qr', apk, '.'], { cwd: payload });

  assert.throws(() => androidCapabilitiesFor(apk), /manifest|APK/i);
  await rm(root, { recursive: true });
});

test('Android validation rejects a compiled manifest with the production bundle identifier', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    'com.magneton.mdreader',
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /invalid manifest or x86_64 APK/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a native library whose declared size exceeds the bound', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setDeclaredSize(
      artifact.apk,
      'lib/x86_64/libmdreader_app_lib.so',
      2_048,
    );
    assert.throws(
      () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
          ) => string
        )(artifact.apk, SOURCE_COMMIT, 1_024),
      /declared native library exceeds the artifact safety bound/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation stops an actual native-library stream at the bound', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.concat([Buffer.from(buildManifest()), Buffer.alloc(2_048, 0x61)]),
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setDeclaredSize(
      artifact.apk,
      'lib/x86_64/libmdreader_app_lib.so',
      512,
    );
    assert.throws(
      () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
          ) => string
        )(artifact.apk, SOURCE_COMMIT, 1_024),
      /native library stream exceeds the artifact safety bound/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects too many x86_64 native libraries before scanning them', async () => {
  const extraLibraries = Array.from(
    { length: 64 },
    (_, index) => [`libdependency_${index}.so`, Buffer.alloc(0)] as const,
  );
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    extraLibraries,
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /native library count exceeds the artifact safety bound/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects aggregate declared native-library bytes', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [['libdependency.so', Buffer.from('dependency')]],
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setDeclaredSize(
      artifact.apk,
      'lib/x86_64/libmdreader_app_lib.so',
      700,
    );
    await setDeclaredSize(artifact.apk, 'lib/x86_64/libdependency.so', 700);
    assert.throws(
      () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
            maxTotalNativeLibraryBytes: number,
          ) => string
        )(artifact.apk, SOURCE_COMMIT, 1_024, 1_200),
      /aggregate declared native library bytes exceed the artifact safety bound/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation stops aggregate native-library streams at one invocation-wide bound', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.concat([Buffer.from(buildManifest()), Buffer.alloc(700, 0x61)]),
    [['libdependency.so', Buffer.alloc(800, 0x62)]],
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setDeclaredSize(
      artifact.apk,
      'lib/x86_64/libmdreader_app_lib.so',
      500,
    );
    await setDeclaredSize(artifact.apk, 'lib/x86_64/libdependency.so', 500);
    assert.throws(
      () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
            maxTotalNativeLibraryBytes: number,
          ) => string
        )(artifact.apk, SOURCE_COMMIT, 2_048, 1_300),
      /aggregate native library stream exceeds the artifact safety bound/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a ZIP64 sentinel on a non-native central member', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setCentralU32(artifact.apk, 'classes.dex', 42, 0xffffffff);
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ZIP64 APK member is unsupported/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a ZIP64 extra record on a non-native member', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setZip64ExtraRecord(artifact.apk, 'classes.dex');
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ZIP64 APK extra record is unsupported/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a structural ZIP64 EOCD record and locator', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await insertZip64EndStructures(artifact.apk);
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ZIP64 APK metadata is unsupported/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation treats ZIP64 magic inside the opaque EOCD comment as data', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    const comment = Buffer.alloc(12);
    comment.writeUInt32LE(0x06064b50, 0);
    comment.writeUInt32LE(0x07064b50, 4);
    comment.write('opaque', 8, 'utf8');
    await appendEocdComment(artifact.apk, comment);
    assert.equal(
      validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      artifact.apk,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects multidisk metadata on any central member', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await setCentralU16(artifact.apk, 'classes.dex', 34, 1);
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /multidisk APK metadata is unsupported/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a duplicate DEX member name', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [['classee.dex', Buffer.from('dex\n035\0')]],
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await renameZipMember(artifact.apk, 'classee.dex', 'classes.dex');
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /duplicate ZIP member classes\.dex/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a duplicate compiled manifest member name', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [['AndroidManifesu.xml', compiledAndroidManifest()]],
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await renameZipMember(
      artifact.apk,
      'AndroidManifesu.xml',
      'AndroidManifest.xml',
    );
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /duplicate ZIP member AndroidManifest\.xml/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a duplicate native library member name', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [['libmdreader_app_liq.so', Buffer.from('dependency')]],
  );
  const previousPath = process.env.PATH;
  process.env.PATH = `${artifact.tools}:${previousPath ?? ''}`;
  try {
    await renameZipMember(
      artifact.apk,
      'lib/x86_64/libmdreader_app_liq.so',
      'lib/x86_64/libmdreader_app_lib.so',
    );
    assert.throws(
      () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /duplicate ZIP member lib\/x86_64\/libmdreader_app_lib\.so/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});
