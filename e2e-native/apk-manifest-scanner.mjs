import { createHash } from 'node:crypto';
import {
  closeSync,
  createReadStream,
  fstatSync,
  openSync,
  readSync,
} from 'node:fs';
import { TextDecoder } from 'node:util';
import { createInflateRaw } from 'node:zlib';

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
const MAX_DEX_BYTES = 256 * 1024 * 1024;
const MAX_TOTAL_DEX_BYTES = 512 * 1024 * 1024;
const MAX_DEX_FILES = 64;
const MAX_DEX_MAP_ITEMS = 256;
const DEX_MAP_TYPES = new Set([
  0x0000,
  0x0001,
  0x0002,
  0x0003,
  0x0004,
  0x0005,
  0x0006,
  0x0007,
  0x0008,
  0x1000,
  0x1001,
  0x1002,
  0x1003,
  0x2000,
  0x2001,
  0x2002,
  0x2003,
  0x2004,
  0x2005,
  0x2006,
  0xf000,
]);
const DEX_VARIABLE_ITEM_MINIMUM_BYTES = new Map([
  [0x1001, 4],
  [0x1002, 4],
  [0x1003, 4],
  [0x2000, 4],
  [0x2001, 16],
  [0x2002, 2],
  [0x2003, 3],
  [0x2004, 3],
  [0x2005, 1],
  [0x2006, 16],
  [0xf000, 1],
]);
const MAX_EMBEDDED_BUILD_MANIFEST_BYTES = 16 * 1024;
const MAX_OTHER_MEMBER_BYTES = 64 * 1024 * 1024;
const MAX_TOTAL_OTHER_BYTES = 256 * 1024 * 1024;
const MAX_COMPRESSION_OVERHEAD_BYTES = 64 * 1024;
const ELF64_HEADER_BYTES = 64;
const ELF64_PROGRAM_HEADER_BYTES = 56;
const MAX_ELF_PROGRAM_HEADERS = 128;
const MAX_ELF_DYNAMIC_BYTES = 64 * 1024;
const MAX_ELF_PREFIX_BYTES =
  ELF64_HEADER_BYTES + ELF64_PROGRAM_HEADER_BYTES * MAX_ELF_PROGRAM_HEADERS;
const ZIP_FLAG_ENCRYPTED = 0x0001;
const ZIP_FLAG_COMPRESSION_OPTION_1 = 0x0002;
const ZIP_FLAG_COMPRESSION_OPTION_2 = 0x0004;
const ZIP_FLAG_DATA_DESCRIPTOR = 0x0008;
const ZIP_FLAG_UTF8 = 0x0800;
const ZIP_ALWAYS_SUPPORTED_FLAGS = ZIP_FLAG_DATA_DESCRIPTOR | ZIP_FLAG_UTF8;
const ZIP_DEFLATE_ONLY_FLAGS =
  ZIP_FLAG_COMPRESSION_OPTION_1 | ZIP_FLAG_COMPRESSION_OPTION_2;
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

function validateMemberPath(name) {
  if (
    name.includes('\\') ||
    name.startsWith('/') ||
    /^[A-Za-z]:/.test(name)
  ) {
    fail(`APK ZIP member path is unsafe: ${name}`);
  }
  const parts = name.split('/');
  const isDirectory = parts.at(-1) === '';
  if (isDirectory) parts.pop();
  if (
    parts.length === 0 ||
    parts.some((part) => part.length === 0 || part === '.' || part === '..')
  ) {
    fail(`APK ZIP member path is unsafe: ${name}`);
  }
  return isDirectory;
}

function validateMemberEncoding(name, flags, method) {
  if ((flags & ZIP_FLAG_ENCRYPTED) !== 0) {
    fail('encrypted APK members are unsupported');
  }
  if (method !== 0 && method !== 8) {
    fail(`APK member ${name} uses an unsupported compression method`);
  }
  const allowedFlags =
    ZIP_ALWAYS_SUPPORTED_FLAGS | (method === 8 ? ZIP_DEFLATE_ONLY_FLAGS : 0);
  if ((flags & ~allowedFlags) !== 0) {
    fail(`APK member ${name} uses unsupported ZIP flags`);
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

function inspectArchive(handle, limits) {
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
    const isDirectory = validateMemberPath(name);
    validateMemberEncoding(name, flags, method);
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
      isDirectory,
    });
    offset += recordLength;
  }
  if (offset !== centralEnd) fail('APK ZIP central directory is invalid');

  let localMetadataBytes = 0;
  const localRegions = [];
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
    const localCrc32 = local.readUInt32LE(14);
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
    const usesDataDescriptor = (localFlags & 0x0008) !== 0;
    if (
      usesDataDescriptor &&
      (localCrc32 !== 0 ||
        localCompressedSize !== 0 ||
        localUncompressedSize !== 0)
    ) {
      fail('APK ZIP data descriptor local CRC and sizes must be zero');
    }
    if (
      !usesDataDescriptor &&
      (localCrc32 !== descriptor.crc32 ||
        localCompressedSize !== descriptor.compressedSize ||
        localUncompressedSize !== descriptor.uncompressedSize)
    ) {
      fail('APK ZIP local and central member CRC or sizes disagree');
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
    let regionEnd = dataEnd;
    if (usesDataDescriptor) {
      const first = readExactly(handle, 4, dataEnd).readUInt32LE(0);
      const hasSignature = first === 0x08074b50;
      const descriptorLength = hasSignature ? 16 : 12;
      const dataDescriptor = readExactly(handle, descriptorLength, dataEnd);
      const valueOffset = hasSignature ? 4 : 0;
      const describedCrc32 = dataDescriptor.readUInt32LE(valueOffset);
      const describedCompressedSize = dataDescriptor.readUInt32LE(valueOffset + 4);
      const describedUncompressedSize = dataDescriptor.readUInt32LE(valueOffset + 8);
      if (
        describedCrc32 !== descriptor.crc32 ||
        describedCompressedSize !== descriptor.compressedSize ||
        describedUncompressedSize !== descriptor.uncompressedSize
      ) {
        fail('APK ZIP data descriptor disagrees with the central directory');
      }
      regionEnd = checkedAdd(
        dataEnd,
        descriptorLength,
        'APK ZIP data descriptor exceeds the artifact safety bound',
      );
      if (regionEnd > centralOffset) {
        fail('APK ZIP data descriptor exceeds the artifact safety bound');
      }
    }
    descriptor.dataOffset = dataOffset;
    localRegions.push({
      start: descriptor.localOffset,
      end: regionEnd,
      name: descriptor.name,
    });
  }
  localRegions.sort((left, right) => left.start - right.start);
  for (let index = 1; index < localRegions.length; index += 1) {
    if (localRegions[index].start < localRegions[index - 1].end) {
      fail(
        `APK ZIP local member regions overlap: ${localRegions[index - 1].name} and ${localRegions[index].name}`,
      );
    }
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
  const selected = new Set([...manifests, ...dexEntries, ...nativeLibraries]);
  const otherEntries = descriptors.filter((descriptor) => !selected.has(descriptor));
  if (manifests.length !== 1) fail('compiled AndroidManifest.xml is missing');
  if (dexEntries.length === 0) fail('compiled DEX application code is missing');
  if (dexEntries.length > MAX_DEX_FILES) {
    fail('Android DEX file count exceeds the artifact safety bound');
  }
  if (nativeLibraries.length === 0) fail('x86_64 emulator native library is missing');
  if (nativeLibraries.length > limits.maximumLibraries) {
    fail('Android native library count exceeds the artifact safety bound');
  }
  if (manifests[0].uncompressedSize > MAX_MANIFEST_BYTES) {
    fail('compiled AndroidManifest.xml exceeds the artifact safety bound');
  }
  if (
    manifests[0].compressedSize >
    MAX_MANIFEST_BYTES + MAX_COMPRESSION_OVERHEAD_BYTES
  ) {
    fail('compressed AndroidManifest.xml exceeds the artifact safety bound');
  }

  let declaredNativeBytes = 0;
  let declaredDexBytes = 0;
  let declaredOtherBytes = 0;
  let compressedWorkBytes = 0;
  for (const descriptor of descriptors) {
    compressedWorkBytes = checkedAdd(
      compressedWorkBytes,
      descriptor.compressedSize,
      'Android compressed archive work exceeds the artifact safety bound',
    );
  }
  for (const dex of dexEntries) {
    if (dex.uncompressedSize > MAX_DEX_BYTES) {
      fail('Android DEX file exceeds the artifact safety bound');
    }
    if (dex.compressedSize > MAX_DEX_BYTES + MAX_COMPRESSION_OVERHEAD_BYTES) {
      fail('compressed Android DEX file exceeds the artifact safety bound');
    }
    declaredDexBytes = checkedAdd(
      declaredDexBytes,
      dex.uncompressedSize,
      'Android aggregate DEX bytes exceed the artifact safety bound',
    );
    if (declaredDexBytes > MAX_TOTAL_DEX_BYTES) {
      fail('Android aggregate DEX bytes exceed the artifact safety bound');
    }
  }
  for (const library of nativeLibraries) {
    if (library.uncompressedSize > limits.maximumNative) {
      fail('Android declared native library exceeds the artifact safety bound');
    }
    declaredNativeBytes = checkedAdd(
      declaredNativeBytes,
      library.uncompressedSize,
      'Android aggregate declared native library bytes exceed the artifact safety bound',
    );
    if (declaredNativeBytes > limits.maximumAggregateNative) {
      fail('Android aggregate declared native library bytes exceed the artifact safety bound');
    }
    const maximumCompressedLibrary = checkedAdd(
      limits.maximumNative,
      MAX_COMPRESSION_OVERHEAD_BYTES,
      'Android compressed native library safety bound is invalid',
    );
    if (library.compressedSize > maximumCompressedLibrary) {
      fail('Android compressed native library exceeds the artifact safety bound');
    }
  }
  for (const descriptor of otherEntries) {
    if (descriptor.uncompressedSize > MAX_OTHER_MEMBER_BYTES) {
      fail('Android non-executable member exceeds the artifact safety bound');
    }
    declaredOtherBytes = checkedAdd(
      declaredOtherBytes,
      descriptor.uncompressedSize,
      'Android aggregate declared non-executable member bytes exceed the artifact safety bound',
    );
    if (declaredOtherBytes > MAX_TOTAL_OTHER_BYTES) {
      fail(
        'Android aggregate declared non-executable member bytes exceed the artifact safety bound',
      );
    }
    const maximumCompressedMember = checkedAdd(
      MAX_OTHER_MEMBER_BYTES,
      MAX_COMPRESSION_OVERHEAD_BYTES,
      'Android compressed non-executable member safety bound is invalid',
    );
    if (descriptor.compressedSize > maximumCompressedMember) {
      fail('Android compressed non-executable member exceeds the artifact safety bound');
    }
  }
  const maximumCompressedWork = checkedAdd(
    checkedAdd(
      limits.maximumAggregateNative,
      MAX_MANIFEST_BYTES,
      'Android compressed archive work safety bound is invalid',
    ),
    checkedAdd(
      checkedAdd(
        MAX_TOTAL_DEX_BYTES,
        MAX_TOTAL_OTHER_BYTES,
        'Android compressed archive work safety bound is invalid',
      ),
      descriptors.length * MAX_COMPRESSION_OVERHEAD_BYTES,
      'Android compressed archive work safety bound is invalid',
    ),
    'Android compressed archive work safety bound is invalid',
  );
  if (compressedWorkBytes > maximumCompressedWork) {
    fail('Android compressed archive work exceeds the artifact safety bound');
  }
  return {
    size,
    manifest: manifests[0],
    dexEntries,
    nativeLibraries,
    otherEntries,
  };
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

async function scanMember(apkPath, descriptor, kind, totals, limits) {
  validateMemberEncoding(descriptor.name, descriptor.flags, descriptor.method);
  const perMemberMaximum =
    kind === 'manifest'
      ? MAX_MANIFEST_BYTES
      : kind === 'dex'
        ? MAX_DEX_BYTES
        : kind === 'native'
          ? limits.maximumNative
          : MAX_OTHER_MEMBER_BYTES;
  let memberBytes = 0;
  let crc = 0xffffffff;
  const chunks = kind === 'manifest' ? [] : null;
  const dex = kind === 'dex' ? new DexStreamValidator(descriptor.name) : null;
  const elf = kind === 'native' ? new ElfStreamValidator(descriptor.name) : null;
  let compressedBytesConsumed =
    descriptor.method === 0 ? descriptor.compressedSize : null;

  const consume = (chunk) => {
    const streamLabel =
      kind === 'native'
        ? 'Android native library'
        : kind === 'dex'
          ? 'Android DEX'
          : kind === 'other'
            ? 'Android non-executable member'
            : 'compiled AndroidManifest.xml';
    memberBytes = checkedAdd(
      memberBytes,
      chunk.length,
      `${streamLabel} stream exceeds the artifact safety bound`,
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
          : kind === 'dex'
            ? 'Android DEX stream exceeds the artifact safety bound'
            : kind === 'other'
              ? 'Android non-executable member stream exceeds the artifact safety bound'
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
      if (totals.native > limits.maximumAggregateNative) {
        fail('Android aggregate native library stream exceeds the artifact safety bound');
      }
    } else if (kind === 'dex') {
      totals.dex = checkedAdd(
        totals.dex,
        chunk.length,
        'Android aggregate DEX stream exceeds the artifact safety bound',
      );
      if (totals.dex > MAX_TOTAL_DEX_BYTES) {
        fail('Android aggregate DEX stream exceeds the artifact safety bound');
      }
    } else if (kind === 'other') {
      totals.other = checkedAdd(
        totals.other,
        chunk.length,
        'Android aggregate non-executable member stream exceeds the artifact safety bound',
      );
      if (totals.other > MAX_TOTAL_OTHER_BYTES) {
        fail(
          'Android aggregate non-executable member stream exceeds the artifact safety bound',
        );
      }
    }
    crc = updateCrc32(crc, chunk);
    if (chunks) chunks.push(Buffer.from(chunk));
    dex?.push(chunk);
    elf?.push(chunk);
  };

  if (descriptor.compressedSize > 0) {
    const source = createReadStream(apkPath, {
      start: descriptor.dataOffset,
      end: descriptor.dataOffset + descriptor.compressedSize - 1,
    });
    const inflater = descriptor.method === 8 ? createInflateRaw() : null;
    const output = inflater === null ? source : source.pipe(inflater);
    try {
      for await (const chunk of output) consume(chunk);
      if (inflater !== null) compressedBytesConsumed = inflater.bytesWritten;
    } finally {
      output.destroy();
      source.destroy();
    }
  }
  if (compressedBytesConsumed !== descriptor.compressedSize) {
    fail(
      `APK member ${descriptor.name} deflate stream did not consume exactly its declared compressed bytes`,
    );
  }
  if (((crc ^ 0xffffffff) >>> 0) !== descriptor.crc32) {
    fail(`APK member ${descriptor.name} checksum is invalid`);
  }
  if (
    descriptor.isDirectory &&
    (memberBytes !== 0 ||
      descriptor.compressedSize !== 0 ||
      descriptor.uncompressedSize !== 0)
  ) {
    fail(`APK ZIP directory member ${descriptor.name} is not empty`);
  }
  dex?.finish(memberBytes);
  const elfResult = elf?.finish(memberBytes);
  return {
    bytes: chunks ? Buffer.concat(chunks, memberBytes) : null,
    embeddedManifest: elfResult?.embeddedManifest ?? null,
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

function updateAdler32(state, bytes) {
  const modulus = 65_521;
  let a = state.a;
  let b = state.b;
  for (const byte of bytes) {
    a = (a + byte) % modulus;
    b = (b + a) % modulus;
  }
  state.a = a;
  state.b = b;
}

function copyStreamRange(target, targetOffset, rangeOffset, chunk, chunkOffset) {
  const start = Math.max(rangeOffset, chunkOffset);
  const end = Math.min(rangeOffset + target.byteLength, chunkOffset + chunk.byteLength);
  if (start >= end) return;
  chunk.copy(
    target,
    targetOffset + start - rangeOffset,
    start - chunkOffset,
    end - chunkOffset,
  );
}

class DexStreamValidator {
  constructor(name) {
    this.name = name;
    this.offset = 0;
    this.header = Buffer.alloc(112);
    this.headerReady = false;
    this.mapOffset = null;
    this.mapCapture = Buffer.alloc(4 + MAX_DEX_MAP_ITEMS * 12);
    this.signature = createHash('sha1');
    this.adler = { a: 1, b: 0 };
    this.sections = [];
  }

  push(chunk) {
    const chunkOffset = this.offset;
    const chunkEnd = checkedAdd(
      chunkOffset,
      chunk.byteLength,
      `APK member ${this.name} DEX stream exceeds the artifact safety bound`,
    );
    if (chunkEnd > 32) {
      this.signature.update(chunk.subarray(Math.max(0, 32 - chunkOffset)));
    }
    if (chunkEnd > 12) {
      updateAdler32(this.adler, chunk.subarray(Math.max(0, 12 - chunkOffset)));
    }
    copyStreamRange(this.header, 0, 0, chunk, chunkOffset);
    this.offset = chunkEnd;
    if (!this.headerReady && this.offset >= this.header.byteLength) {
      this.initializeHeader();
    }
    if (this.mapOffset !== null) {
      copyStreamRange(this.mapCapture, 0, this.mapOffset, chunk, chunkOffset);
    }
  }

  initializeHeader() {
    if (
      !this.header.subarray(0, 4).equals(Buffer.from([0x64, 0x65, 0x78, 0x0a])) ||
      this.header[7] !== 0
    ) {
      fail(`APK member ${this.name} has invalid DEX magic`);
    }
    const version = this.header.subarray(4, 7).toString('ascii');
    if (!['035', '037', '038', '039', '040'].includes(version)) {
      fail(`APK member ${this.name} has unsupported DEX version ${version}`);
    }
    if (
      this.header.readUInt32LE(36) !== this.header.byteLength ||
      this.header.readUInt32LE(40) !== 0x12345678
    ) {
      fail(`APK member ${this.name} has an invalid DEX header`);
    }
    const fileSize = this.header.readUInt32LE(32);
    const dataSize = this.header.readUInt32LE(104);
    const dataOffset = this.header.readUInt32LE(108);
    const dataEnd = checkedAdd(
      dataOffset,
      dataSize,
      `APK member ${this.name} has an invalid DEX data section`,
    );
    if (
      fileSize < this.header.byteLength ||
      dataOffset < this.header.byteLength ||
      dataOffset % 4 !== 0 ||
      dataEnd !== fileSize
    ) {
      fail(`APK member ${this.name} has an invalid DEX data section`);
    }
    const linkSize = this.header.readUInt32LE(44);
    const linkOffset = this.header.readUInt32LE(48);
    if (
      (linkSize === 0) !== (linkOffset === 0) ||
      (linkSize > 0 &&
        (linkOffset < dataOffset ||
          checkedAdd(
            linkOffset,
            linkSize,
            `APK member ${this.name} has an invalid DEX link section`,
          ) > fileSize))
    ) {
      fail(`APK member ${this.name} has an invalid DEX link section`);
    }
    const fixedSections = [
      { type: 0x0001, label: 'string identifiers', sizeAt: 56, offsetAt: 60, width: 4 },
      { type: 0x0002, label: 'type identifiers', sizeAt: 64, offsetAt: 68, width: 4 },
      { type: 0x0003, label: 'prototype identifiers', sizeAt: 72, offsetAt: 76, width: 12 },
      { type: 0x0004, label: 'field identifiers', sizeAt: 80, offsetAt: 84, width: 8 },
      { type: 0x0005, label: 'method identifiers', sizeAt: 88, offsetAt: 92, width: 8 },
      { type: 0x0006, label: 'class definitions', sizeAt: 96, offsetAt: 100, width: 32 },
    ];
    let previousEnd = this.header.byteLength;
    for (const section of fixedSections) {
      const size = this.header.readUInt32LE(section.sizeAt);
      const offset = this.header.readUInt32LE(section.offsetAt);
      if ((size === 0) !== (offset === 0)) {
        fail(`APK member ${this.name} has an invalid DEX section: ${section.label}`);
      }
      if (size === 0) {
        this.sections.push({ type: section.type, size, offset });
        continue;
      }
      const byteLength = size * section.width;
      if (!Number.isSafeInteger(byteLength)) {
        fail(`APK member ${this.name} has an invalid DEX section: ${section.label}`);
      }
      const end = checkedAdd(
        offset,
        byteLength,
        `APK member ${this.name} has an invalid DEX section: ${section.label}`,
      );
      if (
        offset % 4 !== 0 ||
        offset < previousEnd ||
        end > dataOffset
      ) {
        fail(`APK member ${this.name} has an invalid DEX section: ${section.label}`);
      }
      previousEnd = end;
      this.sections.push({ type: section.type, size, offset });
    }
    this.mapOffset = this.header.readUInt32LE(52);
    if (
      this.mapOffset < dataOffset ||
      this.mapOffset % 4 !== 0 ||
      this.mapOffset + 4 > fileSize
    ) {
      fail(`APK member ${this.name} has an invalid DEX map`);
    }
    this.headerReady = true;
  }

  finish(streamedBytes) {
    if (!this.headerReady) {
      fail(`APK member ${this.name} has an invalid DEX header`);
    }
    if (this.header.readUInt32LE(32) !== streamedBytes) {
      fail(`APK member ${this.name} has an invalid DEX header`);
    }
    if (!this.signature.digest().equals(this.header.subarray(12, 32))) {
      fail(`APK member ${this.name} has an invalid DEX signature (SHA-1)`);
    }
    const checksum = (((this.adler.b << 16) | this.adler.a) >>> 0);
    if (checksum !== this.header.readUInt32LE(8)) {
      fail(`APK member ${this.name} has an invalid DEX checksum (Adler32)`);
    }
    const availableMapBytes = streamedBytes - this.mapOffset;
    if (availableMapBytes < 4) {
      fail(`APK member ${this.name} has an invalid DEX map`);
    }
    const count = this.mapCapture.readUInt32LE(0);
    if (count < 2 || count > MAX_DEX_MAP_ITEMS) {
      fail(`APK member ${this.name} has an invalid DEX map`);
    }
    const mapBytes = checkedAdd(
      4,
      count * 12,
      `APK member ${this.name} has an invalid DEX map`,
    );
    if (mapBytes > availableMapBytes) {
      fail(`APK member ${this.name} has an invalid DEX map`);
    }
    const items = new Map();
    let previousOffset = -1;
    for (let index = 0; index < count; index += 1) {
      const itemOffset = 4 + index * 12;
      const type = this.mapCapture.readUInt16LE(itemOffset);
      const unused = this.mapCapture.readUInt16LE(itemOffset + 2);
      const size = this.mapCapture.readUInt32LE(itemOffset + 4);
      const offset = this.mapCapture.readUInt32LE(itemOffset + 8);
      if (
        unused !== 0 ||
        size === 0 ||
        !DEX_MAP_TYPES.has(type) ||
        items.has(type) ||
        offset <= previousOffset ||
        offset >= streamedBytes ||
        (type > 0x0000 && type < 0x1000 && offset < this.header.byteLength) ||
        (type >= 0x1000 && offset < this.header.readUInt32LE(108))
      ) {
        fail(`APK member ${this.name} has an invalid DEX map`);
      }
      items.set(type, { size, offset });
      previousOffset = offset;
    }
    const headerItem = items.get(0x0000);
    const mapItem = items.get(0x1000);
    if (
      headerItem?.size !== 1 ||
      headerItem.offset !== 0 ||
      mapItem?.size !== 1 ||
      mapItem.offset !== this.mapOffset
    ) {
      fail(`APK member ${this.name} has an invalid DEX map`);
    }
    for (const section of this.sections) {
      const item = items.get(section.type);
      if (
        section.size === 0
          ? item !== undefined
          : item?.size !== section.size || item.offset !== section.offset
      ) {
        fail(`APK member ${this.name} has a DEX map that disagrees with its header`);
      }
    }
    for (const [type, width] of [
      [0x0007, 4],
      [0x0008, 8],
    ]) {
      const item = items.get(type);
      if (
        item !== undefined &&
        checkedAdd(
          item.offset,
          item.size * width,
          `APK member ${this.name} has an invalid DEX identifier section`,
        ) > this.header.readUInt32LE(108)
      ) {
        fail(`APK member ${this.name} has an invalid DEX identifier section`);
      }
    }
    const orderedItems = [...items.entries()];
    for (let index = 0; index < orderedItems.length; index += 1) {
      const [type, item] = orderedItems[index];
      const minimumItemBytes = DEX_VARIABLE_ITEM_MINIMUM_BYTES.get(type);
      if (minimumItemBytes === undefined) continue;
      const nextOffset =
        orderedItems[index + 1]?.[1].offset ?? streamedBytes;
      const availableBytes = nextOffset - item.offset;
      const minimumBytes = item.size * minimumItemBytes;
      if (
        !Number.isSafeInteger(minimumBytes) ||
        minimumBytes > availableBytes
      ) {
        fail(
          `APK member ${this.name} has a DEX variable section outside bounds`,
        );
      }
    }
  }
}

function readElfU64(name, bytes, offset) {
  const value = bytes.readBigUInt64LE(offset);
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    fail(`APK member ${name} has an ELF program header outside bounds`);
  }
  return Number(value);
}

function parseX86_64ElfLayout(name, prefix, streamedBytes = null) {
  if (
    prefix.byteLength < 64 ||
    !prefix.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))
  ) {
    fail(`APK member ${name} has invalid ELF magic`);
  }
  if (
    prefix[4] !== 2 ||
    prefix[5] !== 1 ||
    prefix[6] !== 1 ||
    prefix.readUInt16LE(16) !== 3 ||
    prefix.readUInt32LE(20) !== 1 ||
    prefix.readUInt16LE(52) !== ELF64_HEADER_BYTES
  ) {
    fail(
      `APK member ${name} is not a supported 64-bit little-endian ELF shared object`,
    );
  }
  if (prefix.readUInt16LE(18) !== 0x003e) {
    fail(`APK member ${name} has the wrong ELF machine for x86_64`);
  }
  const programOffset = readElfU64(name, prefix, 32);
  const programEntrySize = prefix.readUInt16LE(54);
  const programCount = prefix.readUInt16LE(56);
  const programBytes = programEntrySize * programCount;
  const programEnd = checkedAdd(
    programOffset,
    programBytes,
    `APK member ${name} has an ELF program header outside bounds`,
  );
  if (
    programOffset !== ELF64_HEADER_BYTES ||
    programEntrySize !== ELF64_PROGRAM_HEADER_BYTES ||
    programCount === 0 ||
    programCount > MAX_ELF_PROGRAM_HEADERS ||
    !Number.isSafeInteger(programBytes) ||
    (streamedBytes !== null && programEnd > streamedBytes) ||
    prefix.byteLength < programEnd
  ) {
    fail(`APK member ${name} has an invalid ELF program header table`);
  }
  const sectionOffset = readElfU64(name, prefix, 40);
  const sectionEntrySize = prefix.readUInt16LE(58);
  const sectionCount = prefix.readUInt16LE(60);
  if (
    (sectionCount === 0) !== (sectionOffset === 0) ||
    (streamedBytes !== null &&
      sectionCount > 0 &&
      (sectionEntrySize < 64 ||
        checkedAdd(
          sectionOffset,
          sectionEntrySize * sectionCount,
          `APK member ${name} has an ELF section header outside bounds`,
        ) > streamedBytes))
  ) {
    fail(`APK member ${name} has an ELF section header outside bounds`);
  }
  const loadSegments = [];
  const dynamicSegments = [];
  for (let index = 0; index < programCount; index += 1) {
    const offset = programOffset + index * programEntrySize;
    const type = prefix.readUInt32LE(offset);
    const flags = prefix.readUInt32LE(offset + 4);
    const fileOffset = readElfU64(name, prefix, offset + 8);
    const virtualAddress = readElfU64(name, prefix, offset + 16);
    const fileSize = readElfU64(name, prefix, offset + 32);
    const memorySize = readElfU64(name, prefix, offset + 40);
    const alignment = readElfU64(name, prefix, offset + 48);
    const fileEnd = checkedAdd(
      fileOffset,
      fileSize,
      `APK member ${name} has an ELF program header outside bounds`,
    );
    const virtualFileEnd = checkedAdd(
      virtualAddress,
      fileSize,
      `APK member ${name} has an ELF program header outside bounds`,
    );
    if (
      (streamedBytes !== null && fileEnd > streamedBytes) ||
      memorySize < fileSize ||
      (alignment > 1 &&
        (!Number.isInteger(Math.log2(alignment)) ||
          fileOffset % alignment !== virtualAddress % alignment))
    ) {
      fail(`APK member ${name} has an ELF program header outside bounds`);
    }
    const segment = {
      fileStart: fileOffset,
      fileEnd,
      virtualStart: virtualAddress,
      virtualFileEnd,
      flags,
    };
    if (type === 1) loadSegments.push(segment);
    if (type === 2) dynamicSegments.push(segment);
  }
  if (loadSegments.length === 0) {
    fail(`APK member ${name} has no PT_LOAD program header`);
  }
  if (dynamicSegments.length !== 1) {
    fail(`APK member ${name} must have exactly one PT_DYNAMIC program header`);
  }
  const dynamic = dynamicSegments[0];
  const dynamicBytes = dynamic.fileEnd - dynamic.fileStart;
  if (
    dynamicBytes <= 0 ||
    dynamicBytes > MAX_ELF_DYNAMIC_BYTES ||
    dynamicBytes % 16 !== 0 ||
    !loadSegments.some(
      (load) =>
        load.fileStart <= dynamic.fileStart &&
        load.fileEnd >= dynamic.fileEnd &&
        load.virtualStart <= dynamic.virtualStart &&
        load.virtualFileEnd >= dynamic.virtualFileEnd,
    )
  ) {
    fail(`APK member ${name} has a PT_DYNAMIC segment outside PT_LOAD bounds`);
  }
  const loadFileRanges = [];
  for (const segment of [...loadSegments].sort(
    (left, right) => left.fileStart - right.fileStart,
  )) {
    if (segment.fileEnd <= segment.fileStart) continue;
    const previous = loadFileRanges.at(-1);
    if (previous !== undefined && segment.fileStart <= previous.end) {
      previous.end = Math.max(previous.end, segment.fileEnd);
    } else {
      loadFileRanges.push({
        start: segment.fileStart,
        end: segment.fileEnd,
      });
    }
  }
  if (loadFileRanges.length === 0) {
    fail(`APK member ${name} has no file-backed PT_LOAD program header`);
  }
  return { dynamic, loadSegments, loadFileRanges };
}

function dynamicNumber(name, label, value) {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    fail(`APK member ${name} ${label} reference is outside file-backed PT_LOAD`);
  }
  return Number(value);
}

function validateLoadedReference(name, label, address, size, loadSegments) {
  const end = checkedAdd(
    address,
    size,
    `APK member ${name} ${label} reference is outside file-backed PT_LOAD`,
  );
  if (
    !loadSegments.some(
      (segment) =>
        address >= segment.virtualStart &&
        end <= segment.virtualFileEnd &&
        (size > 0
          ? address < segment.virtualFileEnd
          : address <= segment.virtualFileEnd),
    )
  ) {
    fail(
      `APK member ${name} ${label} reference is outside file-backed PT_LOAD`,
    );
  }
}

function validateElfDynamicTable(name, bytes, loadSegments) {
  const entries = new Map();
  let terminated = false;
  for (let offset = 0; offset < bytes.byteLength; offset += 16) {
    const tagValue = bytes.readBigUInt64LE(offset);
    const value = bytes.readBigUInt64LE(offset + 8);
    if (tagValue === 0n) {
      terminated = true;
      for (let padding = offset; padding < bytes.byteLength; padding += 16) {
        if (
          bytes.readBigUInt64LE(padding) !== 0n ||
          bytes.readBigUInt64LE(padding + 8) !== 0n
        ) {
          fail(`APK member ${name} has data after DT_NULL`);
        }
      }
      break;
    }
    if (tagValue > BigInt(Number.MAX_SAFE_INTEGER)) {
      fail(`APK member ${name} has an unsupported ELF dynamic tag`);
    }
    const tag = Number(tagValue);
    const values = entries.get(tag) ?? [];
    values.push(value);
    entries.set(tag, values);
  }
  if (!terminated) fail(`APK member ${name} has no DT_NULL terminator`);

  const one = (tag, label, required = false) => {
    const values = entries.get(tag) ?? [];
    if (values.length > 1) {
      fail(`APK member ${name} has duplicate ${label} entries`);
    }
    if (required && values.length !== 1) {
      fail(`APK member ${name} is missing ${label}`);
    }
    return values[0];
  };
  const number = (tag, label, required = false) => {
    const value = one(tag, label, required);
    return value === undefined ? null : dynamicNumber(name, label, value);
  };
  const boundedPair = (addressTag, sizeTag, label, alignment = 1) => {
    const address = number(addressTag, label);
    const size = number(sizeTag, `${label} size`);
    if ((address === null) !== (size === null)) {
      fail(`APK member ${name} has an incomplete ${label} dynamic reference`);
    }
    if (address === null || size === null) return;
    if (alignment > 1 && size % alignment !== 0) {
      fail(`APK member ${name} has an invalid ${label} size`);
    }
    validateLoadedReference(name, label, address, size, loadSegments);
  };
  const boundedTable = (
    addressTag,
    sizeTag,
    entryTag,
    label,
    expectedEntryBytes,
  ) => {
    const address = number(addressTag, label);
    const size = number(sizeTag, `${label} size`);
    const entrySize = number(entryTag, `${label} entry size`);
    if (
      [address, size, entrySize].some((value) => value === null) &&
      [address, size, entrySize].some((value) => value !== null)
    ) {
      fail(`APK member ${name} has an incomplete ${label} dynamic reference`);
    }
    if (address === null || size === null || entrySize === null) return;
    if (entrySize !== expectedEntryBytes || size % entrySize !== 0) {
      fail(`APK member ${name} has an invalid ${label} entry size`);
    }
    validateLoadedReference(name, label, address, size, loadSegments);
  };

  const stringTable = number(5, 'DT_STRTAB', true);
  const stringBytes = number(10, 'DT_STRSZ', true);
  validateLoadedReference(
    name,
    'DT_STRTAB',
    stringTable,
    stringBytes,
    loadSegments,
  );
  const symbolTable = number(6, 'DT_SYMTAB', true);
  const symbolEntryBytes = number(11, 'DT_SYMENT', true);
  if (symbolEntryBytes !== 24) {
    fail(`APK member ${name} has an invalid DT_SYMENT`);
  }
  validateLoadedReference(name, 'DT_SYMTAB', symbolTable, 24, loadSegments);

  for (const tag of [1, 14, 15, 29]) {
    for (const value of entries.get(tag) ?? []) {
      const offset = dynamicNumber(name, 'dynamic string', value);
      if (offset >= stringBytes) {
        fail(`APK member ${name} has a dynamic string outside DT_STRTAB`);
      }
    }
  }

  boundedTable(7, 8, 9, 'DT_RELA', 24);
  boundedTable(17, 18, 19, 'DT_REL', 16);
  boundedTable(36, 35, 37, 'DT_RELR', 8);
  boundedPair(25, 27, 'DT_INIT_ARRAY', 8);
  boundedPair(26, 28, 'DT_FINI_ARRAY', 8);
  boundedPair(32, 33, 'DT_PREINIT_ARRAY', 8);
  boundedPair(0x6000000f, 0x60000010, 'DT_ANDROID_REL');
  boundedPair(0x60000011, 0x60000012, 'DT_ANDROID_RELA');

  const jumpAddress = number(23, 'DT_JMPREL');
  const jumpBytes = number(2, 'DT_PLTRELSZ');
  const jumpKind = number(20, 'DT_PLTREL');
  if (
    [jumpAddress, jumpBytes, jumpKind].some((value) => value === null) &&
    [jumpAddress, jumpBytes, jumpKind].some((value) => value !== null)
  ) {
    fail(`APK member ${name} has an incomplete DT_JMPREL dynamic reference`);
  }
  if (jumpAddress !== null && jumpBytes !== null && jumpKind !== null) {
    const entryBytes = jumpKind === 7 ? 24 : jumpKind === 17 ? 16 : 0;
    if (entryBytes === 0 || jumpBytes % entryBytes !== 0) {
      fail(`APK member ${name} has an invalid DT_JMPREL entry size`);
    }
    validateLoadedReference(
      name,
      'DT_JMPREL',
      jumpAddress,
      jumpBytes,
      loadSegments,
    );
  }

  for (const [tag, label, minimumBytes] of [
    [3, 'DT_PLTGOT', 8],
    [4, 'DT_HASH', 8],
    [0x6ffffef5, 'DT_GNU_HASH', 16],
    [0x6ffffff0, 'DT_VERSYM', 2],
  ]) {
    const address = number(tag, label);
    if (address !== null) {
      validateLoadedReference(
        name,
        label,
        address,
        minimumBytes,
        loadSegments,
      );
    }
  }
  for (const [tag, label] of [
    [12, 'DT_INIT'],
    [13, 'DT_FINI'],
  ]) {
    const address = number(tag, label);
    if (address !== null) {
      validateLoadedReference(name, label, address, 1, loadSegments);
    }
  }
  for (const [addressTag, countTag, label, minimumBytes] of [
    [0x6ffffffc, 0x6ffffffd, 'DT_VERDEF', 20],
    [0x6ffffffe, 0x6fffffff, 'DT_VERNEED', 16],
  ]) {
    const address = number(addressTag, label);
    const count = number(countTag, `${label} count`);
    if ((address === null) !== (count === null)) {
      fail(`APK member ${name} has an incomplete ${label} dynamic reference`);
    }
    if (address !== null && count !== null) {
      validateLoadedReference(
        name,
        label,
        address,
        count * minimumBytes,
        loadSegments,
      );
    }
  }
}

class ElfStreamValidator {
  constructor(name) {
    this.name = name;
    this.offset = 0;
    this.prefix = Buffer.alloc(0);
    this.layout = null;
    this.dynamicCapture = null;
    this.dynamicCapturedBytes = 0;
    this.loadScanners = [];
    this.fedThrough = 0;
  }

  push(chunk) {
    const chunkOffset = this.offset;
    this.offset = checkedAdd(
      this.offset,
      chunk.byteLength,
      `APK member ${this.name} ELF stream exceeds the artifact safety bound`,
    );
    if (this.prefix.byteLength < MAX_ELF_PREFIX_BYTES) {
      this.prefix = Buffer.concat([
        this.prefix,
        chunk.subarray(
          0,
          Math.min(
            chunk.byteLength,
            MAX_ELF_PREFIX_BYTES - this.prefix.byteLength,
          ),
        ),
      ]);
    }
    const wasReady = this.layout !== null;
    if (!wasReady) this.tryInitialize();
    if (this.layout === null) return;
    if (!wasReady) {
      this.feed(this.prefix, 0);
      this.fedThrough = this.prefix.byteLength;
    }
    if (this.offset > this.fedThrough) {
      const start = Math.max(0, this.fedThrough - chunkOffset);
      this.feed(chunk.subarray(start), chunkOffset + start);
      this.fedThrough = this.offset;
    }
  }

  tryInitialize() {
    if (this.prefix.byteLength < ELF64_HEADER_BYTES) return;
    const programOffset = readElfU64(this.name, this.prefix, 32);
    const programEntrySize = this.prefix.readUInt16LE(54);
    const programCount = this.prefix.readUInt16LE(56);
    const requiredBytes =
      programOffset + programEntrySize * programCount;
    if (!Number.isSafeInteger(requiredBytes)) {
      fail(`APK member ${this.name} has an invalid ELF program header table`);
    }
    if (this.prefix.byteLength < requiredBytes) return;
    this.layout = parseX86_64ElfLayout(this.name, this.prefix);
    const dynamicBytes =
      this.layout.dynamic.fileEnd - this.layout.dynamic.fileStart;
    this.dynamicCapture = Buffer.alloc(dynamicBytes);
    this.loadScanners = this.layout.loadFileRanges.map((range) => ({
      ...range,
      scanner: new EmbeddedBuildManifestScanner(),
    }));
  }

  feed(chunk, chunkOffset) {
    const dynamic = this.layout.dynamic;
    const dynamicStart = Math.max(dynamic.fileStart, chunkOffset);
    const dynamicEnd = Math.min(
      dynamic.fileEnd,
      chunkOffset + chunk.byteLength,
    );
    if (dynamicStart < dynamicEnd) {
      chunk.copy(
        this.dynamicCapture,
        dynamicStart - dynamic.fileStart,
        dynamicStart - chunkOffset,
        dynamicEnd - chunkOffset,
      );
      this.dynamicCapturedBytes += dynamicEnd - dynamicStart;
    }
    for (const load of this.loadScanners) {
      const start = Math.max(load.start, chunkOffset);
      const end = Math.min(load.end, chunkOffset + chunk.byteLength);
      if (start < end) {
        load.scanner.push(
          chunk.subarray(start - chunkOffset, end - chunkOffset),
        );
      }
    }
  }

  finish(streamedBytes) {
    if (this.layout === null) this.tryInitialize();
    if (this.layout === null) {
      fail(`APK member ${this.name} has an invalid ELF program header table`);
    }
    const layout = parseX86_64ElfLayout(
      this.name,
      this.prefix,
      streamedBytes,
    );
    if (this.dynamicCapturedBytes !== this.dynamicCapture.byteLength) {
      fail(`APK member ${this.name} has a truncated PT_DYNAMIC segment`);
    }
    validateElfDynamicTable(
      this.name,
      this.dynamicCapture,
      layout.loadSegments,
    );
    const manifests = this.loadScanners
      .map((load) => load.scanner.finish())
      .filter((manifest) => manifest !== null);
    if (manifests.length > 1) {
      fail('artifact has an ambiguous embedded E2E build manifest');
    }
    return { embeddedManifest: manifests[0] ?? null };
  }
}

export async function scanAndroidArchive(apk, limits) {
  if (
    typeof apk !== 'string' ||
    apk.length === 0 ||
    !limits ||
    !Number.isSafeInteger(limits.maximumNative) ||
    limits.maximumNative <= 0 ||
    !Number.isSafeInteger(limits.maximumAggregateNative) ||
    limits.maximumAggregateNative <= 0 ||
    !Number.isSafeInteger(limits.maximumLibraries) ||
    limits.maximumLibraries <= 0
  ) {
    fail('invalid Android archive scanner arguments');
  }
  const handle = openSync(apk, 'r');
  try {
    const archive = inspectArchive(handle, limits);
    const totals = {
      decompressed: 0,
      native: 0,
      dex: 0,
      other: 0,
      maximumDecompressed: checkedAdd(
        checkedAdd(
          limits.maximumAggregateNative,
          MAX_MANIFEST_BYTES,
          'Android aggregate decompressed safety bound is invalid',
        ),
        checkedAdd(
          MAX_TOTAL_DEX_BYTES,
          MAX_TOTAL_OTHER_BYTES,
          'Android aggregate decompressed safety bound is invalid',
        ),
        'Android aggregate decompressed safety bound is invalid',
      ),
    };
    const manifest = await scanMember(
      apk,
      archive.manifest,
      'manifest',
      totals,
      limits,
    );
    if (!manifest.sizeMatches) {
      fail('APK member AndroidManifest.xml uncompressed size disagrees with its directory');
    }
    const applicationId = parseApplicationId(manifest.bytes);
    const dexFiles = [];
    for (const descriptor of archive.dexEntries) {
      const scanned = await scanMember(apk, descriptor, 'dex', totals, limits);
      if (!scanned.sizeMatches) {
        fail(`APK member ${descriptor.name} uncompressed size disagrees with its directory`);
      }
      dexFiles.push({
        name: descriptor.name,
        declaredSize: descriptor.uncompressedSize,
        streamedBytes: scanned.streamedBytes,
      });
    }
    const nativeLibraries = [];
    let sizeMismatch = null;
    for (const descriptor of archive.nativeLibraries) {
      const scanned = await scanMember(
        apk,
        descriptor,
        'native',
        totals,
        limits,
      );
      nativeLibraries.push({
        name: descriptor.name,
        declaredSize: descriptor.uncompressedSize,
        streamedBytes: scanned.streamedBytes,
        embeddedManifest: scanned.embeddedManifest,
      });
      if (!scanned.sizeMatches && sizeMismatch === null) {
        sizeMismatch = descriptor.name;
      }
    }
    if (sizeMismatch !== null) {
      fail(`APK member ${sizeMismatch} uncompressed size disagrees with its directory`);
    }
    for (const descriptor of archive.otherEntries) {
      const scanned = await scanMember(
        apk,
        descriptor,
        'other',
        totals,
        limits,
      );
      if (!scanned.sizeMatches) {
        fail(
          `APK member ${descriptor.name} uncompressed size disagrees with its directory`,
        );
      }
    }
    return {
      applicationId,
      manifestStreamedBytes: manifest.streamedBytes,
      aggregateStreamedBytes: totals.decompressed,
      dexFiles,
      nativeLibraries,
    };
  } finally {
    closeSync(handle);
  }
}
