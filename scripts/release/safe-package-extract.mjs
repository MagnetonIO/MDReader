#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { gunzipSync } from 'node:zlib';
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
  win32,
} from 'node:path';
import { TextDecoder } from 'node:util';

const TAR_BLOCK_BYTES = 512;
const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024;
const MAX_ENTRIES = 4096;
const MAX_EXTRACTED_BYTES = 256 * 1024 * 1024;
const TOOL_TIMEOUT_MS = 120_000;
const decoder = new TextDecoder('utf-8', { fatal: true });
const WINDOWS_RESERVED =
  /^(?:CON|PRN|AUX|NUL|CLOCK\$|CONIN\$|CONOUT\$|COM[1-9]|LPT[1-9])(?:\.|$)/i;

function fail(message) {
  throw new Error(`safe package extraction refused: ${message}`);
}

function checkedArchive(path) {
  const metadata = lstatSync(path);
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.size < 1 ||
    metadata.size > MAX_ARCHIVE_BYTES
  ) {
    fail(`archive is not a regular file within the ${MAX_ARCHIVE_BYTES}-byte limit`);
  }
}

function decode(bytes, label) {
  try {
    return decoder.decode(bytes);
  } catch {
    fail(`${label} is not valid UTF-8`);
  }
}

function nulTerminated(bytes, label) {
  const nul = bytes.indexOf(0);
  return decode(nul < 0 ? bytes : bytes.subarray(0, nul), label);
}

function tarNumber(header, offset, length, label) {
  const field = header.subarray(offset, offset + length);
  if ((field[0] & 0x80) !== 0) fail(`tar uses unsupported base-256 ${label}`);
  const value = field.toString('ascii').replace(/\0.*$/, '').trim();
  if (value === '') return 0;
  if (!/^[0-7]+$/.test(value)) fail(`tar has invalid ${label}`);
  const parsed = Number.parseInt(value, 8);
  if (!Number.isSafeInteger(parsed)) fail(`tar has unsafe ${label}`);
  return parsed;
}

function tarChecksum(header) {
  const copy = Buffer.from(header);
  copy.fill(0x20, 148, 156);
  const unsigned = copy.reduce((total, byte) => total + byte, 0);
  const signed = copy.reduce(
    (total, byte) => total + (byte > 127 ? byte - 256 : byte),
    0,
  );
  return { unsigned, signed };
}

function canonicalPath(raw, label = 'member path') {
  if (
    raw.length === 0 ||
    raw.includes('\0') ||
    raw.startsWith('/') ||
    raw.startsWith('\\') ||
    isAbsolute(raw) ||
    win32.isAbsolute(raw)
  ) {
    fail(`${label} is absolute or empty: ${JSON.stringify(raw)}`);
  }
  if (raw.includes('\\')) {
    fail(`${label} contains a backslash separator: ${JSON.stringify(raw)}`);
  }
  const normalized = raw.endsWith('/') ? raw.slice(0, -1) : raw;
  const segments = normalized.split('/');
  if (
    segments.length === 0 ||
    segments.some((segment) => segment === '' || segment === '.')
  ) {
    fail(`${label} contains an empty or dot path segment: ${JSON.stringify(raw)}`);
  }
  if (segments.some((segment) => segment === '..')) {
    fail(`${label} contains parent traversal: ${JSON.stringify(raw)}`);
  }
  for (const segment of segments) {
    if (
      !/^[\u0020-\u007e]+$/u.test(segment) ||
      /[\u0000-\u001f\u007f<>:"|?*]/u.test(segment) ||
      /[. ]$/u.test(segment) ||
      WINDOWS_RESERVED.test(segment)
    ) {
      fail(`${label} is outside the portable filename policy: ${JSON.stringify(raw)}`);
    }
  }
  return {
    path: segments.join('/'),
    segments,
    key: segments.map((segment) => segment.toLowerCase()).join('/'),
  };
}

function checkedCount(entries, totalBytes) {
  if (entries.length > MAX_ENTRIES) {
    fail(`entry count exceeds the reviewed ${MAX_ENTRIES}-entry limit`);
  }
  if (!Number.isSafeInteger(totalBytes) || totalBytes > MAX_EXTRACTED_BYTES) {
    fail(
      `declared byte count exceeds the reviewed ${MAX_EXTRACTED_BYTES}-byte limit`,
    );
  }
}

function closeNamespace(entries) {
  const paths = new Map();
  const portable = new Map();
  const kinds = new Map();
  for (const entry of entries) {
    if (paths.has(entry.path)) {
      fail(`duplicate destination ${entry.path}`);
    }
    const collision = portable.get(entry.key);
    if (collision !== undefined && collision !== entry.path) {
      fail(`case-folding collision between ${collision} and ${entry.path}`);
    }
    paths.set(entry.path, entry);
    portable.set(entry.key, entry.path);
    kinds.set(entry.path, entry.kind);
  }
  for (const entry of entries) {
    for (let length = 1; length < entry.segments.length; length += 1) {
      const path = entry.segments.slice(0, length).join('/');
      const key = entry.segments
        .slice(0, length)
        .map((segment) => segment.toLowerCase())
        .join('/');
      const collision = portable.get(key);
      if (collision !== undefined && collision !== path) {
        fail(`case-folding collision between ${collision} and ${path}`);
      }
      portable.set(key, path);
      if (kinds.get(path) === 'file') {
        fail(`member ${entry.path} is beneath non-directory ${path}`);
      }
      if (!kinds.has(path)) kinds.set(path, 'directory');
    }
  }
  return kinds;
}

function parsePax(body) {
  const fields = new Map();
  let offset = 0;
  while (offset < body.byteLength) {
    const space = body.indexOf(0x20, offset);
    if (space < 1) fail('PAX record has no length');
    const encodedLength = body.subarray(offset, space).toString('ascii');
    if (!/^[1-9][0-9]*$/.test(encodedLength)) fail('PAX record length is invalid');
    const length = Number.parseInt(encodedLength, 10);
    if (
      !Number.isSafeInteger(length) ||
      length <= space - offset + 2 ||
      offset + length > body.byteLength ||
      body[offset + length - 1] !== 0x0a
    ) {
      fail('PAX record is truncated');
    }
    const record = decode(
      body.subarray(space + 1, offset + length - 1),
      'PAX record',
    );
    const equals = record.indexOf('=');
    if (equals < 1) fail('PAX record has no key');
    const key = record.slice(0, equals);
    if (fields.has(key)) fail(`PAX record duplicates ${key}`);
    fields.set(key, record.slice(equals + 1));
    offset += length;
  }
  return fields;
}

function parseTar(input) {
  const archive =
    input[0] === 0x1f && input[1] === 0x8b
      ? gunzipSync(input, { maxOutputLength: MAX_EXTRACTED_BYTES })
      : input;
  if (archive.byteLength % TAR_BLOCK_BYTES !== 0) {
    fail('tar length is not a whole number of blocks');
  }
  const entries = [];
  let totalBytes = 0;
  let offset = 0;
  let ended = false;
  let pax = null;
  let longPath = null;
  while (offset + TAR_BLOCK_BYTES <= archive.byteLength) {
    const header = archive.subarray(offset, offset + TAR_BLOCK_BYTES);
    if (header.every((byte) => byte === 0)) {
      if (
        offset + TAR_BLOCK_BYTES * 2 > archive.byteLength ||
        !archive
          .subarray(offset, offset + TAR_BLOCK_BYTES * 2)
          .every((byte) => byte === 0) ||
        !archive.subarray(offset).every((byte) => byte === 0)
      ) {
        fail('tar has an invalid end marker');
      }
      ended = true;
      break;
    }
    const expectedChecksum = tarNumber(header, 148, 8, 'checksum');
    const actualChecksum = tarChecksum(header);
    if (
      expectedChecksum !== actualChecksum.unsigned &&
      expectedChecksum !== actualChecksum.signed
    ) {
      fail('tar header checksum is invalid');
    }
    const magic = header.subarray(257, 263).toString('binary');
    const version = header.subarray(263, 265).toString('binary');
    if (
      !(
        (magic === 'ustar\0' && version === '00') ||
        (magic === 'ustar ' && version === ' \0')
      )
    ) {
      fail('tar is not in an authorized ustar format');
    }
    const size = tarNumber(header, 124, 12, 'member size');
    const bodyOffset = offset + TAR_BLOCK_BYTES;
    const padded = Math.ceil(size / TAR_BLOCK_BYTES) * TAR_BLOCK_BYTES;
    const next = bodyOffset + padded;
    const type = String.fromCharCode(header[156] || 0x30);
    if (type === '0' || type === '\0') {
      totalBytes += size;
      checkedCount(entries, totalBytes);
    }
    if (next > archive.byteLength) fail('tar member body is truncated');
    const body = archive.subarray(bodyOffset, bodyOffset + size);
    if (type === 'x') {
      if (pax !== null) fail('tar has stacked PAX headers');
      pax = parsePax(body);
      offset = next;
      continue;
    }
    if (type === 'g') fail('tar global PAX headers are not authorized');
    if (type === 'L') {
      if (longPath !== null) fail('tar has stacked GNU long-name headers');
      longPath = nulTerminated(body, 'GNU long name');
      offset = next;
      continue;
    }
    if (['1', '2', 'K'].includes(type)) fail('tar contains a link member');
    if (['3', '4'].includes(type)) fail('tar contains a device member');
    if (type === '6') fail('tar contains a FIFO member');
    if (!['0', '\0', '5'].includes(type)) {
      fail(`tar member type ${JSON.stringify(type)} is not authorized`);
    }
    if (type === '5' && size !== 0) {
      fail('tar directory member has a body');
    }
    const name = nulTerminated(header.subarray(0, 100), 'tar member name');
    const prefix = nulTerminated(header.subarray(345, 500), 'tar member prefix');
    const headerPath = prefix === '' ? name : `${prefix}/${name}`;
    const rawPath = pax?.get('path') ?? longPath ?? headerPath;
    const paxSize = pax?.get('size');
    if (paxSize !== undefined && Number.parseInt(paxSize, 10) !== size) {
      fail('PAX size does not match the tar header');
    }
    if (
      pax?.has('linkpath') ||
      [...(pax?.keys() ?? [])].some(
        (key) =>
          key.startsWith('GNU.sparse.') ||
          key === 'SCHILY.realsize',
      )
    ) {
      fail('tar contains PAX link or sparse-file metadata');
    }
    const canonical = canonicalPath(rawPath, 'tar member path');
    const mode = tarNumber(header, 100, 8, 'member mode') & 0o777;
    entries.push({
      ...canonical,
      kind: type === '5' ? 'directory' : 'file',
      mode,
      body: type === '5' ? Buffer.alloc(0) : body,
    });
    checkedCount(entries, totalBytes);
    pax = null;
    longPath = null;
    offset = next;
  }
  if (!ended || pax !== null || longPath !== null || entries.length === 0) {
    fail('tar is empty, truncated, or has dangling metadata');
  }
  closeNamespace(entries);
  return entries;
}

function hexField(header, offset, label) {
  const value = header.subarray(offset, offset + 8).toString('ascii');
  if (!/^[0-9a-fA-F]{8}$/.test(value)) fail(`cpio ${label} is invalid`);
  return Number.parseInt(value, 16);
}

function align4(value) {
  return (value + 3) & ~3;
}

function parseCpio(archive) {
  const entries = [];
  let totalBytes = 0;
  let offset = 0;
  let ended = false;
  while (offset + 110 <= archive.byteLength) {
    const header = archive.subarray(offset, offset + 110);
    const magic = header.subarray(0, 6).toString('ascii');
    if (magic !== '070701' && magic !== '070702') {
      fail('cpio is not in newc/crc format');
    }
    const mode = hexField(header, 14, 'mode');
    const nlink = hexField(header, 38, 'link count');
    const size = hexField(header, 54, 'member size');
    const nameSize = hexField(header, 94, 'name size');
    const expectedChecksum = hexField(header, 102, 'checksum');
    if (nameSize < 2) fail('cpio member name is empty');
    const nameOffset = offset + 110;
    const bodyOffset = align4(nameOffset + nameSize);
    const next = align4(bodyOffset + size);
    if (
      next > archive.byteLength ||
      archive[nameOffset + nameSize - 1] !== 0
    ) {
      fail('cpio member is truncated');
    }
    const rawPath = decode(
      archive.subarray(nameOffset, nameOffset + nameSize - 1),
      'cpio member name',
    );
    if (rawPath === 'TRAILER!!!') {
      if (size !== 0) fail('cpio trailer has a body');
      ended = true;
      break;
    }
    const fileType = mode & 0o170000;
    if (
      fileType === 0o120000 ||
      (fileType === 0o100000 && nlink !== 1) ||
      (fileType === 0o040000 && nlink < 1)
    ) {
      fail('cpio contains a link member or invalid link count');
    }
    if (fileType === 0o020000 || fileType === 0o060000) {
      fail('cpio contains a device member');
    }
    if (fileType !== 0o100000 && fileType !== 0o040000) {
      fail('cpio contains an unauthorized special member');
    }
    const body = archive.subarray(bodyOffset, bodyOffset + size);
    if (
      magic === '070702' &&
      body.reduce((sum, byte) => (sum + byte) >>> 0, 0) !== expectedChecksum
    ) {
      fail('cpio member checksum is invalid');
    }
    const canonical = canonicalPath(rawPath, 'cpio member path');
    entries.push({
      ...canonical,
      kind: fileType === 0o040000 ? 'directory' : 'file',
      mode: mode & 0o777,
      body: fileType === 0o040000 ? Buffer.alloc(0) : body,
    });
    totalBytes += fileType === 0o100000 ? size : 0;
    checkedCount(entries, totalBytes);
    offset = next;
  }
  if (!ended || entries.length === 0) fail('cpio is empty or has no trailer');
  closeNamespace(entries);
  return entries;
}

function sevenZipRecords(archivePath) {
  const output = execFileSync(
    '7z',
    ['l', '-slt', '-ba', '--', archivePath],
    {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      timeout: TOOL_TIMEOUT_MS,
      killSignal: 'SIGKILL',
    },
  );
  const records = [];
  let record = {};
  for (const line of `${output}\n`.split(/\r?\n/)) {
    if (line === '') {
      if (Object.keys(record).length > 0) records.push(record);
      record = {};
      continue;
    }
    const separator = line.indexOf(' = ');
    if (separator < 1) fail('7z emitted an unparseable member listing');
    const key = line.slice(0, separator);
    if (record[key] !== undefined) fail(`7z listing duplicates field ${key}`);
    record[key] = line.slice(separator + 3);
  }
  if (records.length === 0) fail('7z archive has no listed members');
  const entries = [];
  let totalBytes = 0;
  for (const record of records) {
    if (
      (record['Symbolic Link'] ?? '') !== '' ||
      (record['Hard Link'] ?? '') !== '' ||
      /^[Ll]/.test(String(record.Attributes ?? '').trim())
    ) {
      fail('7z archive contains a link member');
    }
    if (
      record.Anti === '+' ||
      record.Encrypted === '+' ||
      record['Character Device'] !== undefined ||
      record['Block Device'] !== undefined
    ) {
      fail('7z archive contains an unauthorized special member');
    }
    const canonical = canonicalPath(record.Path ?? '', '7z member path');
    const directory =
      record.Folder === '+' || String(record.Attributes ?? '').startsWith('D');
    const size = Number(record.Size ?? (directory ? '0' : ''));
    if (!Number.isSafeInteger(size) || size < 0 || (directory && size !== 0)) {
      fail(`7z member ${canonical.path} has an invalid size`);
    }
    entries.push({
      ...canonical,
      kind: directory ? 'directory' : 'file',
      mode: directory ? 0o755 : 0o644,
      size,
    });
    totalBytes += directory ? 0 : size;
    checkedCount(entries, totalBytes);
  }
  closeNamespace(entries);
  return entries;
}

function materialize(entries, outputRoot, bodyForEntry) {
  if (existsSync(outputRoot)) {
    fail(`output root already exists: ${outputRoot}`);
  }
  const parent = dirname(outputRoot);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  const staging = mkdtempSync(join(parent, `.${basename(outputRoot)}.extracting-`));
  chmodSync(staging, 0o700);
  try {
    const directories = new Map();
    for (const entry of entries) {
      for (let length = 1; length < entry.segments.length; length += 1) {
        const path = entry.segments.slice(0, length).join('/');
        if (!directories.has(path)) directories.set(path, 0o755);
      }
      if (entry.kind === 'directory') directories.set(entry.path, entry.mode);
    }
    for (const [path] of [...directories].sort(
      ([left], [right]) =>
        left.split('/').length - right.split('/').length ||
        left.localeCompare(right),
    )) {
      mkdirSync(join(staging, ...path.split('/')), {
        recursive: false,
        mode: 0o700,
      });
    }
    for (const entry of entries) {
      if (entry.kind !== 'file') continue;
      const destination = join(staging, ...entry.segments);
      const body = bodyForEntry(entry);
      writeFileSync(destination, body, { flag: 'wx', mode: 0o600 });
      chmodSync(destination, entry.mode);
    }
    for (const [path, mode] of [...directories].sort(
      ([left], [right]) => right.split('/').length - left.split('/').length,
    )) {
      chmodSync(join(staging, ...path.split('/')), mode);
    }
    scanRoot(staging);
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
  try {
    renameSync(staging, outputRoot);
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
}

function scanRoot(rootInput) {
  const root = realpathSync(rootInput);
  const rootPrefix = `${root}${sep}`;
  const seen = new Map();
  let entries = 0;
  let bytes = 0;
  const walk = (directory) => {
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name);
      const metadata = lstatSync(path);
      const relativePath = relative(root, path).split(sep).join('/');
      const canonical = canonicalPath(relativePath, 'materialized path');
      const collision = seen.get(canonical.key);
      if (collision !== undefined && collision !== canonical.path) {
        fail(
          `post-extraction case-folding collision between ${collision} and ${canonical.path}`,
        );
      }
      seen.set(canonical.key, canonical.path);
      entries += 1;
      if (metadata.isSymbolicLink()) {
        fail(`post-extraction symbolic link or escape at ${relativePath}`);
      }
      if (!metadata.isDirectory() && !metadata.isFile()) {
        fail(`post-extraction special file at ${relativePath}`);
      }
      const real = realpathSync(path);
      if (real !== root && !real.startsWith(rootPrefix)) {
        fail(`post-extraction path escapes its root: ${relativePath}`);
      }
      if (metadata.isDirectory()) walk(path);
      else bytes += metadata.size;
      checkedCount({ length: entries }, bytes);
    }
  };
  walk(root);
  return { entries, bytes };
}

function extractSevenZip(archivePath, outputRoot) {
  const entries = sevenZipRecords(archivePath);
  if (existsSync(outputRoot)) {
    fail(`output root already exists: ${outputRoot}`);
  }
  const parent = dirname(outputRoot);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  const staging = mkdtempSync(join(parent, `.${basename(outputRoot)}.extracting-`));
  chmodSync(staging, 0o700);
  try {
    execFileSync(
      '7z',
      ['x', '-y', `-o${staging}`, '--', archivePath],
      {
        cwd: staging,
        stdio: ['ignore', 'ignore', 'pipe'],
        maxBuffer: 64 * 1024 * 1024,
        timeout: TOOL_TIMEOUT_MS,
        killSignal: 'SIGKILL',
      },
    );
    scanRoot(staging);
    const expectedFiles = entries
      .filter((entry) => entry.kind === 'file')
      .sort((left, right) => left.path.localeCompare(right.path));
    const actualFiles = [];
    const collect = (directory) => {
      for (const name of readdirSync(directory).sort()) {
        const path = join(directory, name);
        const metadata = lstatSync(path);
        if (metadata.isDirectory()) collect(path);
        else if (metadata.isFile()) {
          actualFiles.push({
            path: relative(staging, path).split(sep).join('/'),
            size: metadata.size,
          });
        }
      }
    };
    collect(staging);
    actualFiles.sort((left, right) => left.path.localeCompare(right.path));
    if (
      actualFiles.length !== expectedFiles.length ||
      expectedFiles.some(
        (entry, index) =>
          actualFiles[index]?.path !== entry.path ||
          actualFiles[index]?.size !== entry.size,
      )
    ) {
      fail('7z materialized inventory differs from its validated listing');
    }
    renameSync(staging, outputRoot);
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
}

function extract(mode, archivePath, outputRoot) {
  checkedArchive(archivePath);
  if (mode === 'tar') {
    const archive = readFileSync(archivePath);
    const entries = parseTar(archive);
    materialize(entries, outputRoot, (entry) => entry.body);
  } else if (mode === 'deb') {
    const tarBody = execFileSync('dpkg-deb', ['--fsys-tarfile', archivePath], {
      maxBuffer: MAX_EXTRACTED_BYTES,
      timeout: TOOL_TIMEOUT_MS,
      killSignal: 'SIGKILL',
    });
    const entries = parseTar(tarBody);
    materialize(entries, outputRoot, (entry) => entry.body);
  } else if (mode === 'rpm') {
    const cpio = execFileSync('rpm2cpio', [archivePath], {
      maxBuffer: MAX_EXTRACTED_BYTES,
      timeout: TOOL_TIMEOUT_MS,
      killSignal: 'SIGKILL',
    });
    const entries = parseCpio(cpio);
    materialize(entries, outputRoot, (entry) => entry.body);
  } else if (mode === 'cpio') {
    const archive = readFileSync(archivePath);
    const entries = parseCpio(archive);
    materialize(entries, outputRoot, (entry) => entry.body);
  } else if (mode === '7z') {
    extractSevenZip(archivePath, outputRoot);
  } else {
    fail(`unsupported extraction mode ${mode}`);
  }
}

function main() {
  const [mode, input, output] = process.argv.slice(2);
  if (mode === 'scan' && input !== undefined && output === undefined) {
    scanRoot(resolve(input));
    return;
  }
  if (
    !['tar', '7z', 'deb', 'rpm', 'cpio'].includes(mode) ||
    input === undefined ||
    output === undefined
  ) {
    throw new Error(
      'usage: safe-package-extract.mjs <tar|7z|deb|rpm|cpio> <archive> <output> | scan <root>',
    );
  }
  extract(mode, resolve(input), resolve(output));
}

try {
  main();
} catch (error) {
  process.stderr.write(
    `${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
}
