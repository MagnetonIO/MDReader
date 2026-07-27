import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
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

function adler32(bytes: Buffer): number {
  const modulus = 65_521;
  let a = 1;
  let b = 0;
  for (const byte of bytes) {
    a = (a + byte) % modulus;
    b = (b + a) % modulus;
  }
  return ((b << 16) | a) >>> 0;
}

function sealDex(dex: Buffer): Buffer {
  createHash('sha1').update(dex.subarray(32)).digest().copy(dex, 12);
  dex.writeUInt32LE(adler32(dex.subarray(12)), 8);
  return dex;
}

function dexFile(version = '035'): Buffer {
  const dex = Buffer.alloc(140);
  dex.write(`dex\n${version}\0`, 0, 'ascii');
  dex.writeUInt32LE(dex.byteLength, 32);
  dex.writeUInt32LE(112, 36);
  dex.writeUInt32LE(0x12345678, 40);
  dex.writeUInt32LE(112, 52);
  dex.writeUInt32LE(28, 104);
  dex.writeUInt32LE(112, 108);
  dex.writeUInt32LE(2, 112);
  dex.writeUInt16LE(0x0000, 116);
  dex.writeUInt32LE(1, 120);
  dex.writeUInt32LE(0, 124);
  dex.writeUInt16LE(0x1000, 128);
  dex.writeUInt32LE(1, 132);
  dex.writeUInt32LE(112, 136);
  return sealDex(dex);
}

function elfLibrary(payload: Buffer, machine = 0x3e, validMagic = true): Buffer {
  const programHeaderOffset = 64;
  const programHeaderSize = 56;
  const programHeaderCount = 2;
  const dynamicOffset =
    programHeaderOffset + programHeaderSize * programHeaderCount;
  const dynamic = Buffer.alloc(16);
  const fileSize = dynamicOffset + dynamic.byteLength + payload.byteLength;
  const header = Buffer.alloc(fileSize);
  header.set(validMagic ? [0x7f, 0x45, 0x4c, 0x46] : [0, 0, 0, 0], 0);
  header[4] = 2;
  header[5] = 1;
  header[6] = 1;
  header.writeUInt16LE(3, 16);
  header.writeUInt16LE(machine, 18);
  header.writeUInt32LE(1, 20);
  header.writeBigUInt64LE(BigInt(programHeaderOffset), 32);
  header.writeUInt16LE(64, 52);
  header.writeUInt16LE(programHeaderSize, 54);
  header.writeUInt16LE(programHeaderCount, 56);

  header.writeUInt32LE(1, programHeaderOffset);
  header.writeUInt32LE(4, programHeaderOffset + 4);
  header.writeBigUInt64LE(0n, programHeaderOffset + 8);
  header.writeBigUInt64LE(0n, programHeaderOffset + 16);
  header.writeBigUInt64LE(0n, programHeaderOffset + 24);
  header.writeBigUInt64LE(BigInt(fileSize), programHeaderOffset + 32);
  header.writeBigUInt64LE(BigInt(fileSize), programHeaderOffset + 40);
  header.writeBigUInt64LE(4_096n, programHeaderOffset + 48);

  const dynamicProgramHeader = programHeaderOffset + programHeaderSize;
  header.writeUInt32LE(2, dynamicProgramHeader);
  header.writeUInt32LE(4, dynamicProgramHeader + 4);
  header.writeBigUInt64LE(BigInt(dynamicOffset), dynamicProgramHeader + 8);
  header.writeBigUInt64LE(BigInt(dynamicOffset), dynamicProgramHeader + 16);
  header.writeBigUInt64LE(BigInt(dynamicOffset), dynamicProgramHeader + 24);
  header.writeBigUInt64LE(BigInt(dynamic.byteLength), dynamicProgramHeader + 32);
  header.writeBigUInt64LE(BigInt(dynamic.byteLength), dynamicProgramHeader + 40);
  header.writeBigUInt64LE(8n, dynamicProgramHeader + 48);

  dynamic.copy(header, dynamicOffset);
  payload.copy(header, dynamicOffset + dynamic.byteLength);
  return header;
}

const exec = promisify(execFile);

async function fakeAndroidArtifact(
  nativeLibrary: Buffer,
  extraNativeLibraries: ReadonlyArray<readonly [string, Buffer]> = [],
  extraFiles: ReadonlyArray<readonly [string, Buffer]> = [],
  manifestPackage = E2E_BUNDLE_IDENTIFIER,
  nativeMachine = 0x3e,
  dex = dexFile(),
  validElfMagic = true,
  nativeLibraryFile?: Buffer,
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
  await writeFile(join(payload, 'classes.dex'), dex);
  await writeFile(
    join(payload, 'lib', 'x86_64', 'libmdreader_app_lib.so'),
    nativeLibraryFile ??
      elfLibrary(nativeLibrary, nativeMachine, validElfMagic),
  );
  for (const [name, bytes] of extraNativeLibraries) {
    await writeFile(join(payload, 'lib', 'x86_64', name), elfLibrary(bytes));
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

type DescriptorMode = 'valid' | 'missing' | 'forged';

interface StoredZipEntry {
  name: string;
  bytes: Buffer;
  descriptor?: DescriptorMode;
}

function crc32(bytes: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ ((crc & 1) === 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function storedDescriptorLocalRecord(entry: StoredZipEntry): Buffer {
  const name = Buffer.from(entry.name, 'utf8');
  const checksum = crc32(entry.bytes);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0x0008, 6);
  local.writeUInt16LE(0, 8);
  local.writeUInt16LE(name.byteLength, 26);
  const descriptor =
    entry.descriptor === 'missing'
      ? Buffer.alloc(0)
      : (() => {
          const value = Buffer.alloc(16);
          value.writeUInt32LE(0x08074b50, 0);
          value.writeUInt32LE(
            entry.descriptor === 'forged' ? (checksum ^ 0xffffffff) >>> 0 : checksum,
            4,
          );
          value.writeUInt32LE(entry.bytes.byteLength, 8);
          value.writeUInt32LE(entry.bytes.byteLength, 12);
          return value;
        })();
  return Buffer.concat([local, name, entry.bytes, descriptor]);
}

function storedDescriptorZip(entries: StoredZipEntry[]): Buffer {
  const localRecords: Buffer[] = [];
  const centralRecords: Buffer[] = [];
  let localOffset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name, 'utf8');
    const local = storedDescriptorLocalRecord(entry);
    localRecords.push(local);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(CENTRAL_SIGNATURE, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0008, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt32LE(crc32(entry.bytes), 16);
    central.writeUInt32LE(entry.bytes.byteLength, 20);
    central.writeUInt32LE(entry.bytes.byteLength, 24);
    central.writeUInt16LE(name.byteLength, 28);
    central.writeUInt32LE(localOffset, 42);
    centralRecords.push(Buffer.concat([central, name]));
    localOffset += local.byteLength;
  }
  const centralDirectory = Buffer.concat(centralRecords);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(EOCD_SIGNATURE, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralDirectory.byteLength, 12);
  eocd.writeUInt32LE(localOffset, 16);
  return Buffer.concat([...localRecords, centralDirectory, eocd]);
}

async function descriptorAndroidArtifact(
  descriptor: DescriptorMode = 'valid',
  dex = dexFile(),
  extraEntries: StoredZipEntry[] = [],
): Promise<{ apk: string; root: string }> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-android-descriptor-'));
  const apk = join(root, 'descriptor.apk');
  await writeFile(
    apk,
    storedDescriptorZip([
      {
        name: 'AndroidManifest.xml',
        bytes: compiledAndroidManifest(),
        descriptor,
      },
      { name: 'classes.dex', bytes: dex },
      {
        name: 'lib/x86_64/libmdreader_app_lib.so',
        bytes: elfLibrary(Buffer.from(buildManifest())),
      },
      ...extraEntries,
    ]),
  );
  return { apk, root };
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
      const localOffset = bytes.readUInt32LE(offset + 42);
      if ((bytes.readUInt16LE(localOffset + 6) & 0x0008) === 0) {
        bytes.writeUInt32LE(declaredSize, localOffset + 22);
      }
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

async function setLocalU32(
  apk: string,
  member: string,
  fieldOffset: number,
  value: number,
): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, member);
  bytes.writeUInt32LE(value, record.localOffset + fieldOffset);
  await writeFile(apk, bytes);
}

async function setLocalU16(
  apk: string,
  member: string,
  fieldOffset: number,
  value: number,
): Promise<void> {
  const bytes = await readFile(apk);
  const record = zipRecord(bytes, member);
  bytes.writeUInt16LE(value, record.localOffset + fieldOffset);
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
  await assert.rejects(async () => androidCapabilitiesFor(undefined), /MDREADER_ANDROID_APP/);

  const apk = join(tmpdir(), `mdreader-${process.pid}.apk`);
  await writeFile(apk, '');
  await assert.rejects(async () => androidCapabilitiesFor(apk), /empty/);
  await writeFile(apk, 'test fixture');
  await assert.rejects(async () => androidCapabilitiesFor(apk), /APK|ZIP/);
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

  await assert.rejects(async () => androidCapabilitiesFor(apk), /manifest|APK/i);
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /invalid manifest or x86_64 APK/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation runs in-process without a child Node executable', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  const executable = process.execPath;
  process.execPath = join(artifact.root, 'missing-node');
  try {
    assert.equal(
      await validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      artifact.apk,
    );
  } finally {
    process.execPath = executable;
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an invalid DEX magic header', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    Buffer.alloc(112),
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX magic/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an unsupported DEX version', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile('999'),
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX version/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a DEX with a forged Adler-32 checksum', async () => {
  const dex = dexFile();
  dex.writeUInt32LE((dex.readUInt32LE(8) ^ 0xffffffff) >>> 0, 8);
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dex,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX checksum/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a DEX with a forged SHA-1 signature', async () => {
  const dex = dexFile();
  dex[12] ^= 0xff;
  dex.writeUInt32LE(adler32(dex.subarray(12)), 8);
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dex,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX signature/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a DEX map outside the declared data section', async () => {
  const dex = dexFile();
  dex.writeUInt32LE(dex.byteLength - 2, 52);
  sealDex(dex);
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dex,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX map/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a DEX fixed-width section outside the file', async () => {
  const dex = dexFile();
  dex.writeUInt32LE(1, 56);
  dex.writeUInt32LE(dex.byteLength - 2, 60);
  sealDex(dex);
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dex,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /DEX section/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a native library without ELF magic', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile(),
    false,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ELF magic/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an x86_64 ELF that is not a shared object', async () => {
  const library = elfLibrary(Buffer.from(buildManifest()));
  library.writeUInt16LE(2, 16);
  const artifact = await fakeAndroidArtifact(
    Buffer.alloc(0),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile(),
    true,
    library,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ELF shared object/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an ELF shared object without PT_LOAD', async () => {
  const library = elfLibrary(Buffer.from(buildManifest()));
  library.writeUInt32LE(0, 64);
  const artifact = await fakeAndroidArtifact(
    Buffer.alloc(0),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile(),
    true,
    library,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /PT_LOAD/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an ELF shared object without PT_DYNAMIC', async () => {
  const library = elfLibrary(Buffer.from(buildManifest()));
  library.writeUInt32LE(0, 120);
  const artifact = await fakeAndroidArtifact(
    Buffer.alloc(0),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile(),
    true,
    library,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /PT_DYNAMIC/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an ELF program segment outside the library', async () => {
  const library = elfLibrary(Buffer.from(buildManifest()));
  library.writeBigUInt64LE(BigInt(library.byteLength + 1), 64 + 32);
  const artifact = await fakeAndroidArtifact(
    Buffer.alloc(0),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0x3e,
    dexFile(),
    true,
    library,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ELF program header.*bounds/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation binds the build manifest to the MDReader app library', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from('no build manifest'),
    [['libdependency.so', Buffer.from(buildManifest())]],
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /MDReader app library.*build manifest/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a native library for another ELF machine', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [],
    E2E_BUNDLE_IDENTIFIER,
    0xb7,
  );
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /x86_64|ELF machine/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a clear-bit local CRC that disagrees with central metadata', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  try {
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 14, 0);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /local and central.*CRC|metadata disagree/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects clear-bit local sizes that disagree with central metadata', async () => {
  const artifact = await fakeAndroidArtifact(Buffer.from(buildManifest()));
  try {
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 18, 1);
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 22, 1);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /local and central.*size|metadata disagree/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation requires zero local CRC and sizes when a data descriptor is used', async () => {
  const artifact = await descriptorAndroidArtifact();
  try {
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 14, 1);
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 18, 2);
    await setLocalU32(artifact.apk, 'AndroidManifest.xml', 22, 3);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /data descriptor.*local.*zero/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation accepts a bounded signed data descriptor', async () => {
  const artifact = await descriptorAndroidArtifact();
  try {
    assert.equal(
      await validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      artifact.apk,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an unsafe path on an unselected ZIP member', async () => {
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [['aa/evil', Buffer.from('opaque')]],
  );
  try {
    await renameZipMember(artifact.apk, 'aa/evil', '../evil');
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /ZIP member path/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects encryption on an unselected ZIP member', async () => {
  const member = 'assets/opaque.bin';
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [[member, Buffer.from('opaque')]],
  );
  try {
    await setCentralU16(artifact.apk, member, 8, 0x0001);
    await setLocalU16(artifact.apk, member, 6, 0x0001);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /encrypted APK members/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects an unsupported method on an unselected ZIP member', async () => {
  const member = 'assets/opaque.bin';
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [[member, Buffer.from('opaque')]],
  );
  try {
    await setCentralU16(artifact.apk, member, 10, 99);
    await setLocalU16(artifact.apk, member, 8, 99);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /unsupported compression method/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation checks the CRC of an unselected ZIP member', async () => {
  const member = 'assets/opaque.bin';
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [[member, Buffer.from('opaque')]],
  );
  try {
    await setCentralU32(artifact.apk, member, 16, 0);
    await setLocalU32(artifact.apk, member, 14, 0);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /checksum is invalid/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation bounds the declared size of an unselected ZIP member', async () => {
  const member = 'assets/opaque.bin';
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [[member, Buffer.from('opaque')]],
  );
  try {
    await setDeclaredSize(artifact.apk, member, 512 * 1024 * 1024);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /non-executable member exceeds the artifact safety bound/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation bounds aggregate declared unselected ZIP bytes', async () => {
  const members = Array.from(
    { length: 5 },
    (_, index) => `assets/opaque-${index}.bin`,
  );
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    members.map((member) => [member, Buffer.from('opaque')]),
  );
  try {
    for (const member of members) {
      await setDeclaredSize(artifact.apk, member, 60 * 1024 * 1024);
    }
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /aggregate declared non-executable member bytes exceed the artifact safety bound/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation checks the actual size of an unselected ZIP member', async () => {
  const member = 'assets/opaque.bin';
  const artifact = await fakeAndroidArtifact(
    Buffer.from(buildManifest()),
    [],
    [[member, Buffer.from('opaque')]],
  );
  try {
    await setDeclaredSize(artifact.apk, member, 1);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /uncompressed size disagrees with its directory/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a missing data descriptor', async () => {
  const artifact = await descriptorAndroidArtifact('missing');
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /data descriptor/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects a forged data descriptor', async () => {
  const artifact = await descriptorAndroidArtifact('forged');
  try {
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /data descriptor/i,
    );
  } finally {
    await rm(artifact.root, { recursive: true });
  }
});

test('Android validation rejects overlapping local member regions', async () => {
  const inner = {
    name: 'assets/inner',
    bytes: Buffer.from('inner'),
  };
  const dex = Buffer.concat([dexFile(), storedDescriptorLocalRecord(inner)]);
  const artifact = await descriptorAndroidArtifact('valid', dex, [inner]);
  try {
    const bytes = await readFile(artifact.apk);
    const dexRecord = zipRecord(bytes, 'classes.dex');
    const dexLocalExtraLength = bytes.readUInt16LE(dexRecord.localOffset + 28);
    const nestedOffset =
      dexRecord.localOffset +
      30 +
      dexRecord.nameLength +
      dexLocalExtraLength +
      dexFile().byteLength;
    await setCentralU32(artifact.apk, inner.name, 42, nestedOffset);
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /overlap/i,
    );
  } finally {
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
    await assert.rejects(
      async () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
          ) => Promise<string>
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
    await assert.rejects(
      async () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
          ) => Promise<string>
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
            maxTotalNativeLibraryBytes: number,
          ) => Promise<string>
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
    await assert.rejects(
      async () =>
        (
          validateAndroidApk as unknown as (
            apk: string,
            commit: string,
            maxNativeLibraryBytes: number,
            maxTotalNativeLibraryBytes: number,
          ) => Promise<string>
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
      await validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
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
    await assert.rejects(
      async () => validateAndroidApk(artifact.apk, SOURCE_COMMIT),
      /duplicate ZIP member lib\/x86_64\/libmdreader_app_lib\.so/i,
    );
  } finally {
    process.env.PATH = previousPath;
    await rm(artifact.root, { recursive: true });
  }
});
