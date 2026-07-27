import {
  closeSync,
  createReadStream,
  fstatSync,
  openSync,
  readSync,
} from 'node:fs';
import { TextDecoder } from 'node:util';
import { createInflateRaw } from 'node:zlib';

const [apk, maximumNativeText, maximumAggregateNativeText, maximumLibrariesText] =
  process.argv.slice(2);
const maximumNative = Number(maximumNativeText);
const maximumAggregateNative = Number(maximumAggregateNativeText);
const maximumLibraries = Number(maximumLibrariesText);
if (
  !apk ||
  !Number.isSafeInteger(maximumNative) ||
  maximumNative <= 0 ||
  !Number.isSafeInteger(maximumAggregateNative) ||
  maximumAggregateNative <= 0 ||
  !Number.isSafeInteger(maximumLibraries) ||
  maximumLibraries <= 0
) {
  process.stderr.write('invalid Android archive scanner arguments\n');
  process.exit(2);
}

const CENTRAL_SIGNATURE = 0x02014b50;
const EOCD_SIGNATURE = 0x06054b50;
const LOCAL_SIGNATURE = 0x04034b50;
const ZIP64_EOCD_SIGNATURE = 0x06064b50;
const ZIP64_LOCATOR_SIGNATURE = 0x07064b50;
const ZIP64_EXTRA_ID = 0x0001;
const MAX_CENTRAL_DIRECTORY_BYTES = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES = 16_384;
const MAX_MEMBER_NAME_BYTES = 4_096;
const MAX_LOCAL_METADATA_BYTES = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 4 * 1024 * 1024;
const MAX_EMBEDDED_BUILD_MANIFEST_BYTES = 16 * 1024;
const MAX_COMPRESSION_OVERHEAD_BYTES = 64 * 1024;
const BUILD_MANIFEST_PREFIX = Buffer.from('MDREADER_E2E_BUILD_MANIFEST:');
const BUILD_MANIFEST_SUFFIX = Buffer.from(':MDREADER_E2E_BUILD_MANIFEST_END');
const UTF8 = new TextDecoder('utf-8', { fatal: true });

function fail(message) {
  throw new Error(message);
}

function checkedAdd(left, right, detail) {
  const sum = left + right;
  if (!Number.isSafeInteger(sum)) fail(detail);
  return sum;
}

function readExactly(handle, length, position) {
  if (
    !Number.isSafeInteger(length) ||
    length < 0 ||
    !Number.isSafeInteger(position) ||
    position < 0
  ) {
    fail('APK ZIP metadata exceeds the artifact safety bound');
  }
  const bytes = Buffer.alloc(length);
  let offset = 0;
  while (offset < length) {
    const count = readSync(handle, bytes, offset, length - offset, position + offset);
    if (count === 0) fail('APK ZIP metadata is truncated');
    offset += count;
  }
  return bytes;
}

function parseExtraRecords(extra) {
  let offset = 0;
  while (offset < extra.length) {
    if (offset + 4 > extra.length) fail('APK ZIP extra metadata is truncated');
    const id = extra.readUInt16LE(offset);
    const size = extra.readUInt16LE(offset + 2);
    offset += 4;
    if (offset + size > extra.length) fail('APK ZIP extra metadata is truncated');
    if (id === ZIP64_EXTRA_ID) fail('ZIP64 APK extra record is unsupported');
    offset += size;
  }
}

function decodeMemberName(bytes) {
  try {
    const name = UTF8.decode(bytes);
    if (name.length === 0 || name.includes('\0')) fail('APK ZIP member name is invalid');
    return name;
  } catch {
    fail('APK ZIP member name is not valid UTF-8');
  }
}

function findEocd(handle, size) {
  const tailLength = Math.min(size, 65_557);
  const tail = readExactly(handle, tailLength, size - tailLength);
  for (let index = tail.length - 22; index >= 0; index -= 1) {
    if (
      tail.readUInt32LE(index) === EOCD_SIGNATURE &&
      index + 22 + tail.readUInt16LE(index + 20) === tail.length
    ) {
      return {
        bytes: tail.subarray(index, index + 22),
        offset: size - tailLength + index,
      };
    }
  }
  fail('APK ZIP central directory is missing');
}

function inspectArchive(apkPath, handle) {
  const size = fstatSync(handle).size;
  if (!Number.isSafeInteger(size) || size < 22) fail('APK ZIP metadata is truncated');
  const eocd = findEocd(handle, size);
  const diskNumber = eocd.bytes.readUInt16LE(4);
  const centralDisk = eocd.bytes.readUInt16LE(6);
  const entriesOnDisk = eocd.bytes.readUInt16LE(8);
  const entries = eocd.bytes.readUInt16LE(10);
  const centralSize = eocd.bytes.readUInt32LE(12);
  const centralOffset = eocd.bytes.readUInt32LE(16);
  if (
    entries === 0xffff ||
    entriesOnDisk === 0xffff ||
    centralSize === 0xffffffff ||
    centralOffset === 0xffffffff
  ) {
    fail('ZIP64 APK metadata is unsupported');
  }
  if (diskNumber !== 0 || centralDisk !== 0 || entriesOnDisk !== entries) {
    fail('multidisk APK metadata is unsupported');
  }
  if (entries === 0 || entries > MAX_ARCHIVE_ENTRIES) {
    fail('APK ZIP entry count exceeds the artifact safety bound');
  }
  if (centralSize > MAX_CENTRAL_DIRECTORY_BYTES) {
    fail('APK ZIP central directory exceeds the artifact safety bound');
  }
  const centralEnd = checkedAdd(
    centralOffset,
    centralSize,
    'APK ZIP central directory exceeds the artifact safety bound',
  );
  if (centralEnd !== eocd.offset) {
    const leadingSignature =
      centralEnd + 4 <= size
        ? readExactly(handle, 4, centralEnd).readUInt32LE(0)
        : null;
    const locatorSignature =
      eocd.offset >= 20
        ? readExactly(handle, 4, eocd.offset - 20).readUInt32LE(0)
        : null;
    if (
      leadingSignature === ZIP64_EOCD_SIGNATURE ||
      locatorSignature === ZIP64_LOCATOR_SIGNATURE
    ) {
      fail('ZIP64 APK metadata is unsupported');
    }
    fail('APK ZIP central directory exceeds the artifact safety bound');
  }

  const descriptors = [];
  const names = new Set();
  let offset = centralOffset;
  for (let index = 0; index < entries; index += 1) {
    if (offset + 46 > centralEnd) fail('APK ZIP central directory is truncated');
    const header = readExactly(handle, 46, offset);
    if (header.readUInt32LE(0) !== CENTRAL_SIGNATURE) {
      fail('APK ZIP central directory is invalid');
    }
    const flags = header.readUInt16LE(8);
    const method = header.readUInt16LE(10);
    const crc32 = header.readUInt32LE(16);
    const compressedSize = header.readUInt32LE(20);
    const uncompressedSize = header.readUInt32LE(24);
    const nameLength = header.readUInt16LE(28);
    const extraLength = header.readUInt16LE(30);
    const commentLength = header.readUInt16LE(32);
    const diskStart = header.readUInt16LE(34);
    const localOffset = header.readUInt32LE(42);
    if (
      compressedSize === 0xffffffff ||
      uncompressedSize === 0xffffffff ||
      localOffset === 0xffffffff ||
      diskStart === 0xffff
    ) {
      fail('ZIP64 APK member is unsupported');
    }
    if (diskStart !== 0) fail('multidisk APK metadata is unsupported');
    if (nameLength === 0 || nameLength > MAX_MEMBER_NAME_BYTES) {
      fail('APK ZIP member name exceeds the artifact safety bound');
    }
    const recordLength = checkedAdd(
      46,
      checkedAdd(
        nameLength,
        checkedAdd(extraLength, commentLength, 'APK ZIP central entry is invalid'),
        'APK ZIP central entry is invalid',
      ),
      'APK ZIP central entry is invalid',
    );
    if (offset + recordLength > centralEnd) {
      fail('APK ZIP central directory entry exceeds the artifact safety bound');
    }
    const nameBytes = readExactly(handle, nameLength, offset + 46);
    const name = decodeMemberName(nameBytes);
    if (names.has(name)) fail(`APK contains duplicate ZIP member ${name}`);
    names.add(name);
    const extra = readExactly(handle, extraLength, offset + 46 + nameLength);
    parseExtraRecords(extra);
    descriptors.push({
      name,
      nameBytes,
      flags,
      method,
      crc32,
      compressedSize,
      uncompressedSize,
      localOffset,
      dataOffset: 0,
    });
    offset += recordLength;
  }
  if (offset !== centralEnd) fail('APK ZIP central directory is invalid');

  let localMetadataBytes = 0;
  for (const descriptor of descriptors) {
    if (descriptor.localOffset + 30 > centralOffset) {
      fail('APK ZIP local member header is outside the data region');
    }
    const local = readExactly(handle, 30, descriptor.localOffset);
    if (local.readUInt32LE(0) !== LOCAL_SIGNATURE) {
      fail('APK ZIP local member header is invalid');
    }
    const localFlags = local.readUInt16LE(6);
    const localMethod = local.readUInt16LE(8);
    const localCompressedSize = local.readUInt32LE(18);
    const localUncompressedSize = local.readUInt32LE(22);
    const localNameLength = local.readUInt16LE(26);
    const localExtraLength = local.readUInt16LE(28);
    if (
      localCompressedSize === 0xffffffff ||
      localUncompressedSize === 0xffffffff
    ) {
      fail('ZIP64 APK member is unsupported');
    }
    if (
      localFlags !== descriptor.flags ||
      localMethod !== descriptor.method ||
      localNameLength !== descriptor.nameBytes.length
    ) {
      fail('APK ZIP local and central member metadata disagree');
    }
    const localMetadataLength = checkedAdd(
      30,
      checkedAdd(
        localNameLength,
        localExtraLength,
        'APK ZIP local metadata exceeds the artifact safety bound',
      ),
      'APK ZIP local metadata exceeds the artifact safety bound',
    );
    localMetadataBytes = checkedAdd(
      localMetadataBytes,
      localMetadataLength,
      'APK ZIP local metadata exceeds the artifact safety bound',
    );
    if (localMetadataBytes > MAX_LOCAL_METADATA_BYTES) {
      fail('APK ZIP local metadata exceeds the artifact safety bound');
    }
    const localNameBytes = readExactly(
      handle,
      localNameLength,
      descriptor.localOffset + 30,
    );
    if (!localNameBytes.equals(descriptor.nameBytes)) {
      fail('APK ZIP local and central member names disagree');
    }
    const localExtra = readExactly(
      handle,
      localExtraLength,
      descriptor.localOffset + 30 + localNameLength,
    );
    parseExtraRecords(localExtra);
    const dataOffset = descriptor.localOffset + localMetadataLength;
    const dataEnd = checkedAdd(
      dataOffset,
      descriptor.compressedSize,
      'APK ZIP member data exceeds the artifact safety bound',
    );
    if (dataEnd > centralOffset) {
      fail('APK ZIP member data exceeds the artifact safety bound');
    }
    descriptor.dataOffset = dataOffset;
  }

  const manifests = descriptors.filter(
    (descriptor) => descriptor.name === 'AndroidManifest.xml',
  );
  const dexEntries = descriptors.filter((descriptor) =>
    /^classes(?:\d+)?\.dex$/.test(descriptor.name),
  );
  const nativeLibraries = descriptors.filter((descriptor) =>
    /^lib\/x86_64\/[^/]+\.so$/.test(descriptor.name),
  );
  if (manifests.length !== 1) fail('compiled AndroidManifest.xml is missing');
  if (dexEntries.length === 0) fail('compiled DEX application code is missing');
  if (nativeLibraries.length === 0) fail('x86_64 emulator native library is missing');
  if (nativeLibraries.length > maximumLibraries) {
    fail('Android native library count exceeds the artifact safety bound');
  }
  if (manifests[0].uncompressedSize > MAX_MANIFEST_BYTES) {
    fail('compiled AndroidManifest.xml exceeds the artifact safety bound');
  }

  let declaredNativeBytes = 0;
  let compressedWorkBytes = manifests[0].compressedSize;
  for (const library of nativeLibraries) {
    if (library.uncompressedSize > maximumNative) {
      fail('Android declared native library exceeds the artifact safety bound');
    }
    declaredNativeBytes = checkedAdd(
      declaredNativeBytes,
      library.uncompressedSize,
      'Android aggregate declared native library bytes exceed the artifact safety bound',
    );
    if (declaredNativeBytes > maximumAggregateNative) {
      fail('Android aggregate declared native library bytes exceed the artifact safety bound');
    }
    const maximumCompressedLibrary = checkedAdd(
      maximumNative,
      MAX_COMPRESSION_OVERHEAD_BYTES,
      'Android compressed native library safety bound is invalid',
    );
    if (library.compressedSize > maximumCompressedLibrary) {
      fail('Android compressed native library exceeds the artifact safety bound');
    }
    compressedWorkBytes = checkedAdd(
      compressedWorkBytes,
      library.compressedSize,
      'Android compressed archive work exceeds the artifact safety bound',
    );
  }
  const maximumCompressedWork = checkedAdd(
    checkedAdd(
      maximumAggregateNative,
      MAX_MANIFEST_BYTES,
      'Android compressed archive work safety bound is invalid',
    ),
    (nativeLibraries.length + 1) * MAX_COMPRESSION_OVERHEAD_BYTES,
    'Android compressed archive work safety bound is invalid',
  );
  if (compressedWorkBytes > maximumCompressedWork) {
    fail('Android compressed archive work exceeds the artifact safety bound');
  }
  return { size, manifest: manifests[0], nativeLibraries };
}

const CRC32_TABLE = Array.from({ length: 256 }, (_, value) => {
  let crc = value;
  for (let bit = 0; bit < 8; bit += 1) {
    crc = (crc >>> 1) ^ ((crc & 1) === 1 ? 0xedb88320 : 0);
  }
  return crc >>> 0;
});

function updateCrc32(crc, bytes) {
  let next = crc;
  for (const byte of bytes) {
    next = CRC32_TABLE[(next ^ byte) & 0xff] ^ (next >>> 8);
  }
  return next >>> 0;
}

class EmbeddedBuildManifestScanner {
  constructor() {
    this.pending = Buffer.alloc(0);
    this.capturing = false;
    this.manifest = null;
  }

  push(bytes) {
    let input = Buffer.concat([this.pending, bytes]);
    this.pending = Buffer.alloc(0);
    while (input.length > 0) {
      if (!this.capturing) {
        const start = input.indexOf(BUILD_MANIFEST_PREFIX);
        if (start < 0) {
          const retained = Math.min(input.length, BUILD_MANIFEST_PREFIX.length - 1);
          this.pending = Buffer.from(input.subarray(input.length - retained));
          return;
        }
        if (this.manifest !== null) {
          fail('artifact has an ambiguous embedded E2E build manifest');
        }
        this.capturing = true;
        input = input.subarray(start);
      }
      const end = input.indexOf(
        BUILD_MANIFEST_SUFFIX,
        BUILD_MANIFEST_PREFIX.length,
      );
      if (end < 0) {
        if (input.length > MAX_EMBEDDED_BUILD_MANIFEST_BYTES) {
          fail('artifact has an invalid embedded E2E build manifest');
        }
        this.pending = Buffer.from(input);
        return;
      }
      const manifestEnd = end + BUILD_MANIFEST_SUFFIX.length;
      this.manifest = Buffer.from(input.subarray(0, manifestEnd));
      this.capturing = false;
      input = input.subarray(manifestEnd);
    }
  }

  finish() {
    if (this.capturing) fail('artifact has an invalid embedded E2E build manifest');
    return this.manifest;
  }
}

async function scanMember(apkPath, descriptor, kind, totals) {
  if ((descriptor.flags & 0x0001) !== 0) fail('encrypted APK members are unsupported');
  if (descriptor.method !== 0 && descriptor.method !== 8) {
    fail(`APK member ${descriptor.name} uses an unsupported compression method`);
  }
  const perMemberMaximum = kind === 'manifest' ? MAX_MANIFEST_BYTES : maximumNative;
  let memberBytes = 0;
  let crc = 0xffffffff;
  const chunks = kind === 'manifest' ? [] : null;
  const embedded = kind === 'native' ? new EmbeddedBuildManifestScanner() : null;

  const consume = (chunk) => {
    memberBytes = checkedAdd(
      memberBytes,
      chunk.length,
      `${kind === 'native' ? 'Android native library' : 'compiled AndroidManifest.xml'} stream exceeds the artifact safety bound`,
    );
    totals.decompressed = checkedAdd(
      totals.decompressed,
      chunk.length,
      'Android aggregate decompressed stream exceeds the artifact safety bound',
    );
    if (memberBytes > perMemberMaximum) {
      fail(
        kind === 'native'
          ? 'Android native library stream exceeds the artifact safety bound'
          : 'compiled AndroidManifest.xml exceeds the artifact safety bound',
      );
    }
    if (totals.decompressed > totals.maximumDecompressed) {
      fail('Android aggregate decompressed stream exceeds the artifact safety bound');
    }
    if (kind === 'native') {
      totals.native = checkedAdd(
        totals.native,
        chunk.length,
        'Android aggregate native library stream exceeds the artifact safety bound',
      );
      if (totals.native > maximumAggregateNative) {
        fail('Android aggregate native library stream exceeds the artifact safety bound');
      }
    }
    crc = updateCrc32(crc, chunk);
    if (chunks) chunks.push(Buffer.from(chunk));
    embedded?.push(chunk);
  };

  if (descriptor.compressedSize > 0) {
    const source = createReadStream(apkPath, {
      start: descriptor.dataOffset,
      end: descriptor.dataOffset + descriptor.compressedSize - 1,
    });
    const output = descriptor.method === 8 ? source.pipe(createInflateRaw()) : source;
    try {
      for await (const chunk of output) consume(chunk);
    } finally {
      output.destroy();
      source.destroy();
    }
  }
  if (((crc ^ 0xffffffff) >>> 0) !== descriptor.crc32) {
    fail(`APK member ${descriptor.name} checksum is invalid`);
  }
  return {
    bytes: chunks ? Buffer.concat(chunks, memberBytes) : null,
    embeddedManifest: embedded?.finish() ?? null,
    sizeMatches: memberBytes === descriptor.uncompressedSize,
    streamedBytes: memberBytes,
  };
}

function readLength8(bytes, offset) {
  if (offset >= bytes.length) fail('Android binary XML string is truncated');
  const first = bytes[offset];
  return (first & 0x80) === 0
    ? { length: first, next: offset + 1 }
    : offset + 1 < bytes.length
      ? { length: ((first & 0x7f) << 8) | bytes[offset + 1], next: offset + 2 }
      : fail('Android binary XML string is truncated');
}

function readLength16(bytes, offset) {
  if (offset + 2 > bytes.length) fail('Android binary XML string is truncated');
  const first = bytes.readUInt16LE(offset);
  if ((first & 0x8000) === 0) return { length: first, next: offset + 2 };
  if (offset + 4 > bytes.length) fail('Android binary XML string is truncated');
  return {
    length: ((first & 0x7fff) << 16) | bytes.readUInt16LE(offset + 2),
    next: offset + 4,
  };
}

function parseStringPool(xml, chunkOffset, headerSize, chunkSize) {
  if (headerSize < 28 || chunkOffset + 28 > xml.length) {
    fail('Android binary XML string pool is invalid');
  }
  const stringCount = xml.readUInt32LE(chunkOffset + 8);
  const styleCount = xml.readUInt32LE(chunkOffset + 12);
  const flags = xml.readUInt32LE(chunkOffset + 16);
  const stringsStart = xml.readUInt32LE(chunkOffset + 20);
  const stylesStart = xml.readUInt32LE(chunkOffset + 24);
  if (stringCount > 65_536 || styleCount > 65_536) {
    fail('Android binary XML string pool exceeds the safety bound');
  }
  const offsetsBytes = checkedAdd(
    stringCount * 4,
    styleCount * 4,
    'Android binary XML string pool is invalid',
  );
  if (
    headerSize + offsetsBytes > chunkSize ||
    stringsStart < headerSize + offsetsBytes ||
    stringsStart >= chunkSize ||
    (stylesStart !== 0 && (stylesStart < stringsStart || stylesStart > chunkSize))
  ) {
    fail('Android binary XML string pool is invalid');
  }
  const utf8 = (flags & 0x00000100) !== 0;
  return (index) => {
    if (!Number.isInteger(index) || index < 0 || index >= stringCount) {
      fail('Android binary XML string index is invalid');
    }
    const relative = xml.readUInt32LE(chunkOffset + headerSize + index * 4);
    let offset = chunkOffset + stringsStart + relative;
    const stringEnd = chunkOffset + (stylesStart === 0 ? chunkSize : stylesStart);
    if (offset >= stringEnd) fail('Android binary XML string is outside its pool');
    if (utf8) {
      const utf16Length = readLength8(xml.subarray(0, stringEnd), offset);
      const byteLength = readLength8(xml.subarray(0, stringEnd), utf16Length.next);
      offset = byteLength.next;
      if (offset + byteLength.length + 1 > stringEnd || xml[offset + byteLength.length] !== 0) {
        fail('Android binary XML string is truncated');
      }
      try {
        return UTF8.decode(xml.subarray(offset, offset + byteLength.length));
      } catch {
        fail('Android binary XML string is not valid UTF-8');
      }
    }
    const length = readLength16(xml.subarray(0, stringEnd), offset);
    offset = length.next;
    const byteLength = length.length * 2;
    if (
      offset + byteLength + 2 > stringEnd ||
      xml.readUInt16LE(offset + byteLength) !== 0
    ) {
      fail('Android binary XML string is truncated');
    }
    return xml.subarray(offset, offset + byteLength).toString('utf16le');
  };
}

function parseApplicationId(xml) {
  if (
    xml.length < 8 ||
    xml.readUInt16LE(0) !== 0x0003 ||
    xml.readUInt16LE(2) < 8 ||
    xml.readUInt32LE(4) !== xml.length
  ) {
    fail('AndroidManifest.xml is not compiled Android binary XML');
  }
  let offset = xml.readUInt16LE(2);
  let strings = null;
  let manifestElements = 0;
  let applicationId = null;
  while (offset < xml.length) {
    if (offset + 8 > xml.length) fail('Android binary XML chunk is truncated');
    const type = xml.readUInt16LE(offset);
    const headerSize = xml.readUInt16LE(offset + 2);
    const chunkSize = xml.readUInt32LE(offset + 4);
    if (
      headerSize < 8 ||
      chunkSize < headerSize ||
      offset + chunkSize > xml.length
    ) {
      fail('Android binary XML chunk is invalid');
    }
    if (type === 0x0001) {
      if (strings !== null) fail('Android binary XML has multiple string pools');
      strings = parseStringPool(xml, offset, headerSize, chunkSize);
    } else if (type === 0x0102) {
      if (strings === null || headerSize < 16 || headerSize + 20 > chunkSize) {
        fail('Android binary XML start element is invalid');
      }
      const extension = offset + headerSize;
      const name = strings(xml.readUInt32LE(extension + 4));
      const attributeStart = xml.readUInt16LE(extension + 8);
      const attributeSize = xml.readUInt16LE(extension + 10);
      const attributeCount = xml.readUInt16LE(extension + 12);
      if (
        attributeCount > 1_024 ||
        attributeSize < 20 ||
        attributeStart < 20 ||
        extension + attributeStart + attributeSize * attributeCount >
          offset + chunkSize
      ) {
        fail('Android binary XML attributes are invalid');
      }
      if (name === 'manifest') {
        manifestElements += 1;
        for (let index = 0; index < attributeCount; index += 1) {
          const attribute =
            extension + attributeStart + index * attributeSize;
          if (strings(xml.readUInt32LE(attribute + 4)) !== 'package') continue;
          const rawValue = xml.readUInt32LE(attribute + 8);
          const typedSize = xml.readUInt16LE(attribute + 12);
          const dataType = xml.readUInt8(attribute + 15);
          const data = xml.readUInt32LE(attribute + 16);
          if (typedSize !== 8) fail('Android binary XML package value is invalid');
          const value =
            rawValue !== 0xffffffff
              ? strings(rawValue)
              : dataType === 0x03
                ? strings(data)
                : null;
          if (!value || applicationId !== null) {
            fail('Android binary XML package value is invalid');
          }
          applicationId = value;
        }
      }
    }
    offset += chunkSize;
  }
  if (offset !== xml.length || manifestElements !== 1 || applicationId === null) {
    fail('Android binary XML package value is missing');
  }
  return applicationId;
}

async function main() {
  const handle = openSync(apk, 'r');
  try {
    const archive = inspectArchive(apk, handle);
    const totals = {
      decompressed: 0,
      native: 0,
      maximumDecompressed: checkedAdd(
        maximumAggregateNative,
        MAX_MANIFEST_BYTES,
        'Android aggregate decompressed safety bound is invalid',
      ),
    };
    const manifest = await scanMember(
      apk,
      archive.manifest,
      'manifest',
      totals,
    );
    if (!manifest.sizeMatches) {
      fail('APK member AndroidManifest.xml uncompressed size disagrees with its directory');
    }
    const applicationId = parseApplicationId(manifest.bytes);
    const nativeLibraries = [];
    let sizeMismatch = null;
    for (const descriptor of archive.nativeLibraries) {
      const scanned = await scanMember(
        apk,
        descriptor,
        'native',
        totals,
      );
      nativeLibraries.push({
        name: descriptor.name,
        declared_size: descriptor.uncompressedSize,
        streamed_bytes: scanned.streamedBytes,
        manifest_base64:
          scanned.embeddedManifest === null
            ? null
            : scanned.embeddedManifest.toString('base64'),
      });
      if (!scanned.sizeMatches && sizeMismatch === null) {
        sizeMismatch = descriptor.name;
      }
    }
    if (sizeMismatch !== null) {
      fail(`APK member ${sizeMismatch} uncompressed size disagrees with its directory`);
    }
    return {
      application_id: applicationId,
      manifest_streamed_bytes: manifest.streamedBytes,
      aggregate_streamed_bytes: totals.decompressed,
      native_libraries: nativeLibraries,
    };
  } finally {
    closeSync(handle);
  }
}

try {
  process.stdout.write(JSON.stringify(await main()));
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 2;
}
