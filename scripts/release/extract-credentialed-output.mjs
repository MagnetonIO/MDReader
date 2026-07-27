#!/usr/bin/env node

import { createHash } from 'node:crypto';
import {
  chmodSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { TextDecoder } from 'node:util';
import { pathToFileURL } from 'node:url';

const BLOCK_BYTES = 512;
const ZERO_BLOCKS_BYTES = BLOCK_BYTES * 2;
const MAX_ARCHIVE_BYTES = 1024 * 1024 * 1024;
const decoder = new TextDecoder('utf-8', { fatal: true });
const WINDOWS_RESERVED_NAMES =
  /^(?:CON|PRN|AUX|NUL|CLOCK\$|CONIN\$|CONOUT\$|COM[1-9¹²³]|LPT[1-9¹²³])$/i;

function fail(message) {
  throw new Error(`credentialed archive ${message}`);
}

function decodeString(header, offset, length, label) {
  const field = header.subarray(offset, offset + length);
  const nul = field.indexOf(0);
  const bytes = nul === -1 ? field : field.subarray(0, nul);
  try {
    return decoder.decode(bytes);
  } catch {
    fail(`contains invalid UTF-8 in its ${label}`);
  }
}

function decodeOctal(header, offset, length, label) {
  const field = header.subarray(offset, offset + length);
  if ((field[0] & 0x80) !== 0) {
    fail(`uses unsupported base-256 ${label}`);
  }
  const encoded = field
    .toString('ascii')
    .replace(/\0.*$/, '')
    .trim();
  if (encoded === '') return 0;
  if (!/^[0-7]+$/.test(encoded)) {
    fail(`contains invalid octal ${label}`);
  }
  const value = Number.parseInt(encoded, 8);
  if (!Number.isSafeInteger(value)) {
    fail(`contains an unsafe ${label}`);
  }
  return value;
}

function checksum(header) {
  const copy = Buffer.from(header);
  copy.fill(0x20, 148, 156);
  const unsigned = copy.reduce((total, byte) => total + byte, 0);
  const signed = copy.reduce(
    (total, byte) => total + (byte > 127 ? byte - 256 : byte),
    0,
  );
  return { unsigned, signed };
}

function memberKind(type) {
  if (type === '\0' || type === '0') return 'file';
  if (type === '2') return 'symlink';
  if (type === '5') return 'directory';
  const descriptions = {
    '1': 'contains an unauthorized hard link',
    '3': 'contains an unauthorized character device',
    '4': 'contains an unauthorized block device',
    '6': 'contains an unauthorized FIFO',
    '7': 'contains an unsupported contiguous-file member',
    S: 'contains an unsupported GNU sparse member',
    x: 'contains an unsupported PAX extended header',
    g: 'contains an unsupported PAX global header',
    L: 'contains an unsupported GNU long-name header',
    K: 'contains an unsupported GNU long-link header',
  };
  fail(
    descriptions[type] ??
      `contains an unsupported archive member type ${JSON.stringify(type)}`,
  );
}

function validateWindowsSegment(segment, context) {
  if (/[\u0000-\u001f\u007f<>:"|?*]/u.test(segment)) {
    fail(`${context} contains a Windows-forbidden character`);
  }
  if (/[. ]$/u.test(segment)) {
    fail(`${context} contains a trailing dot or space`);
  }
  const base = segment.split('.')[0];
  if (WINDOWS_RESERVED_NAMES.test(base)) {
    fail(`${context} contains a Windows reserved name`);
  }
}

function validatePortableAsciiSegment(segment, label) {
  if (!/^[\u0020-\u007e]+$/u.test(segment)) {
    fail(`contains a non-ASCII ${label} segment`);
  }
  if (segment.includes('~')) {
    fail(`${label} segment is outside the portable ASCII filename policy`);
  }
}

function portablePathKey(path) {
  return path
    .split('/')
    .map((segment) => segment.toLowerCase())
    .join('/');
}

function canonicalArchivePath(rawName, kind) {
  if (rawName === '') fail('contains an empty member name');
  if (rawName.startsWith('/') || rawName.startsWith('\\')) {
    fail(`contains an absolute member path: ${rawName}`);
  }
  if (rawName.includes('\\')) {
    fail(`contains a backslash path-separator alias: ${rawName}`);
  }
  const hasTrailingSlash = rawName.endsWith('/');
  if (hasTrailingSlash && kind !== 'directory') {
    fail(`contains a trailing slash alias for a non-directory: ${rawName}`);
  }
  const withoutDirectorySlash = hasTrailingSlash
    ? rawName.slice(0, -1)
    : rawName;
  const segments = withoutDirectorySlash.split('/');
  if (segments.some((segment) => segment === '')) {
    fail(`contains an empty path segment: ${rawName}`);
  }
  if (segments.some((segment) => segment === '.')) {
    fail(`contains a dot path segment: ${rawName}`);
  }
  if (segments.some((segment) => segment === '..')) {
    fail(`contains a parent path segment: ${rawName}`);
  }
  for (const segment of segments) {
    validatePortableAsciiSegment(segment, 'archive path');
    validateWindowsSegment(segment, `member path ${rawName}`);
  }
  const path = segments.join('/');
  if (
    path !== 'credentialed-output.json' &&
    path !== 'release-config.json' &&
    path !== 'bundle' &&
    !path.startsWith('bundle/')
  ) {
    fail(`contains a member outside the closed output namespace: ${rawName}`);
  }
  if (kind === 'symlink' && !path.startsWith('bundle/')) {
    fail(`contains a symbolic link outside the closed bundle namespace: ${rawName}`);
  }
  const portableKey = portablePathKey(path);
  return { path, portableKey, segments };
}

function parseArchive(archive) {
  if (archive.byteLength % BLOCK_BYTES !== 0) {
    fail('length is not a whole number of tar blocks');
  }
  const members = [];
  let offset = 0;
  let foundEnd = false;
  while (offset + BLOCK_BYTES <= archive.byteLength) {
    const header = archive.subarray(offset, offset + BLOCK_BYTES);
    if (header.every((byte) => byte === 0)) {
      if (
        offset + ZERO_BLOCKS_BYTES > archive.byteLength ||
        !archive
          .subarray(offset, offset + ZERO_BLOCKS_BYTES)
          .every((byte) => byte === 0)
      ) {
        fail('does not contain two terminating zero blocks');
      }
      if (!archive.subarray(offset).every((byte) => byte === 0)) {
        fail('contains data after its terminating zero blocks');
      }
      foundEnd = true;
      break;
    }
    const expectedChecksum = decodeOctal(header, 148, 8, 'header checksum');
    const actualChecksum = checksum(header);
    if (
      expectedChecksum !== actualChecksum.unsigned &&
      expectedChecksum !== actualChecksum.signed
    ) {
      fail('contains a header with an invalid checksum');
    }
    const magic = decodeString(header, 257, 6, 'format magic');
    const version = decodeString(header, 263, 2, 'format version');
    if (magic !== 'ustar' || version !== '00') {
      fail('is not in the authorized POSIX ustar format');
    }
    const type = String.fromCharCode(header[156]);
    const kind = memberKind(type);
    const name = decodeString(header, 0, 100, 'member name');
    const prefix = decodeString(header, 345, 155, 'member prefix');
    const rawName = prefix === '' ? name : `${prefix}/${name}`;
    const mode = decodeOctal(header, 100, 8, 'member mode');
    if (mode < 0 || mode > 0o777) {
      fail(`contains unsupported permission bits for ${rawName}`);
    }
    const size = decodeOctal(header, 124, 12, 'member size');
    if (kind !== 'file' && size !== 0) {
      fail(`contains a non-file member with a body: ${rawName}`);
    }
    const bodyStart = offset + BLOCK_BYTES;
    const paddedSize = Math.ceil(size / BLOCK_BYTES) * BLOCK_BYTES;
    const nextOffset = bodyStart + paddedSize;
    if (nextOffset > archive.byteLength) {
      fail(`truncates member data for ${rawName}`);
    }
    const canonical = canonicalArchivePath(rawName, kind);
    const linkname =
      kind === 'symlink'
        ? decodeString(header, 157, 100, 'symbolic-link target')
        : '';
    members.push({
      ...canonical,
      kind,
      mode,
      body: archive.subarray(bodyStart, bodyStart + size),
      linkname,
      rawName,
    });
    offset = nextOffset;
  }
  if (!foundEnd) fail('has no terminating zero blocks');
  if (members.length === 0) fail('is empty');
  return members;
}

function closeNamespace(members) {
  const byPath = new Map();
  const byPortablePath = new Map();
  for (const member of members) {
    if (byPath.has(member.path)) {
      fail(`contains a duplicate canonical archive entry: ${member.rawName}`);
    }
    const portableCollision = byPortablePath.get(member.portableKey);
    if (
      portableCollision !== undefined &&
      portableCollision.path !== member.path
    ) {
      fail(
        `contains a Windows case-folding collision: ${portableCollision.path} and ${member.path}`,
      );
    }
    byPath.set(member.path, member);
    byPortablePath.set(member.portableKey, { path: member.path });
  }

  const knownPaths = new Set(byPath.keys());
  for (const member of members) {
    for (let length = 1; length < member.segments.length; length += 1) {
      const ancestorPath = member.segments.slice(0, length).join('/');
      knownPaths.add(ancestorPath);
      const ancestorPortableKey = portablePathKey(ancestorPath);
      const portableCollision = byPortablePath.get(ancestorPortableKey);
      if (
        portableCollision !== undefined &&
        portableCollision.path !== ancestorPath
      ) {
        fail(
          `contains a Windows case-folding collision: ${portableCollision.path} and ${ancestorPath}`,
        );
      }
      byPortablePath.set(ancestorPortableKey, { path: ancestorPath });
      const ancestor = byPath.get(ancestorPath);
      if (ancestor?.kind === 'symlink') {
        fail(
          `contains an archive member beneath a symbolic link: ${member.rawName}`,
        );
      }
      if (ancestor !== undefined && ancestor.kind !== 'directory') {
        fail(
          `contains an archive member beneath a non-directory: ${member.rawName}`,
        );
      }
    }
  }

  for (const required of [
    'credentialed-output.json',
    'release-config.json',
    'bundle',
  ]) {
    if (!knownPaths.has(required)) {
      fail(`does not contain required namespace entry ${required}`);
    }
  }
  for (const requiredFile of [
    'credentialed-output.json',
    'release-config.json',
  ]) {
    if (byPath.get(requiredFile)?.kind !== 'file') {
      fail(`does not contain ${requiredFile} as a regular file`);
    }
  }
  if (byPath.get('bundle')?.kind !== 'directory') {
    fail('does not contain bundle as a directory');
  }

  const symlinkTargets = new Map();
  for (const member of members) {
    if (member.kind !== 'symlink') continue;
    if (member.linkname === '') {
      fail(`contains an empty symbolic-link target: ${member.rawName}`);
    }
    if (member.linkname.startsWith('/') || member.linkname.startsWith('\\')) {
      fail(`contains an absolute symbolic-link target: ${member.rawName}`);
    }
    if (member.linkname.includes('\\')) {
      fail(
        `contains a backslash path-separator alias in a symbolic-link target: ${member.rawName}`,
      );
    }
    const targetSegments = member.linkname.split('/');
    if (targetSegments.some((segment) => segment === '')) {
      fail(`contains an empty symbolic-link target segment: ${member.rawName}`);
    }
    if (targetSegments.some((segment) => segment === '.')) {
      fail(`contains a dot symbolic-link target segment: ${member.rawName}`);
    }
    for (const segment of targetSegments) {
      if (segment !== '..') {
        validatePortableAsciiSegment(segment, 'symbolic-link target');
        validateWindowsSegment(
          segment,
          `symbolic-link target for ${member.rawName}`,
        );
      }
    }
    symlinkTargets.set(member.path, targetSegments);
  }

  function resolveTarget(link) {
    const remaining = [
      ...link.segments.slice(0, -1),
      ...symlinkTargets.get(link.path),
    ];
    const resolved = [];
    const followed = new Set();
    let hops = 0;
    while (remaining.length > 0) {
      const segment = remaining.shift();
      if (segment === '..') {
        if (resolved.length <= 1) {
          fail(`symbolic-link target escapes the closed bundle root: ${link.rawName}`);
        }
        resolved.pop();
        continue;
      }
      resolved.push(segment);
      if (resolved[0] !== 'bundle') {
        fail(`symbolic-link target escapes the closed bundle root: ${link.rawName}`);
      }
      const path = resolved.join('/');
      if (!knownPaths.has(path)) {
        fail(
          `symbolic-link target is absent from the closed bundle namespace: ${link.rawName}`,
        );
      }
      const nestedTarget = symlinkTargets.get(path);
      if (nestedTarget !== undefined) {
        if (followed.has(path) || hops >= members.length) {
          fail(`contains a symbolic-link cycle: ${link.rawName}`);
        }
        followed.add(path);
        hops += 1;
        resolved.pop();
        remaining.unshift(...nestedTarget);
      }
    }
    if (resolved.length === 0 || resolved[0] !== 'bundle') {
      fail(`symbolic-link target escapes the closed bundle root: ${link.rawName}`);
    }
  }

  for (const member of members) {
    if (member.kind === 'symlink') resolveTarget(member);
  }
  return { byPath, knownPaths };
}

function assertDirectory(path) {
  const metadata = lstatSync(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    fail(`materialization parent is not an exact directory: ${path}`);
  }
}

function lexicalPathOrder(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function materialize(members, namespace, outputRoot) {
  const stagingRoot = mkdtempSync(`${outputRoot}.materializing-`);
  chmodSync(stagingRoot, 0o700);
  try {
    const directoryPaths = [...namespace.knownPaths]
      .filter((path) => {
        const member = namespace.byPath.get(path);
        return member === undefined || member.kind === 'directory';
      })
      .sort((left, right) => {
        const depth = left.split('/').length - right.split('/').length;
        return depth || lexicalPathOrder(left, right);
      });
    for (const path of directoryPaths) {
      const parent = dirname(join(stagingRoot, ...path.split('/')));
      assertDirectory(parent);
      mkdirSync(join(stagingRoot, ...path.split('/')), {
        recursive: false,
        mode: 0o700,
      });
    }

    const files = members
      .filter((member) => member.kind === 'file')
      .sort((left, right) => lexicalPathOrder(left.path, right.path));
    for (const member of files) {
      const path = join(stagingRoot, ...member.segments);
      assertDirectory(dirname(path));
      writeFileSync(path, member.body, { flag: 'wx', mode: 0o600 });
      chmodSync(path, member.mode);
    }

    const symlinks = members
      .filter((member) => member.kind === 'symlink')
      .sort((left, right) => lexicalPathOrder(left.path, right.path));
    for (const member of symlinks) {
      const path = join(stagingRoot, ...member.segments);
      assertDirectory(dirname(path));
      symlinkSync(member.linkname, path);
      if (readlinkSync(path) !== member.linkname) {
        fail(`did not materialize the exact symbolic-link target: ${member.rawName}`);
      }
    }

    for (const path of directoryPaths.reverse()) {
      const member = namespace.byPath.get(path);
      chmodSync(join(stagingRoot, ...path.split('/')), member?.mode ?? 0o755);
    }
    rmSync(outputRoot, { recursive: true, force: true });
    renameSync(stagingRoot, outputRoot);
  } catch (error) {
    rmSync(stagingRoot, { recursive: true, force: true });
    throw error;
  }
}

export function assertCredentialedArchiveSize(size) {
  if (!Number.isSafeInteger(size) || size < 0) {
    fail('has an invalid byte length');
  }
  if (size > MAX_ARCHIVE_BYTES) {
    fail(`exceeds the reviewed ${MAX_ARCHIVE_BYTES}-byte limit`);
  }
}

function verifyHandoff(platform, downloadRootInput) {
  const downloadRoot = realpathSync(downloadRootInput);
  const names = readdirSync(downloadRoot);
  if (names.length !== 1) {
    fail('handoff must contain exactly one hash-addressed artifact');
  }
  const artifactRoot = join(downloadRoot, names[0]);
  const expectedPrefix = `mdreader-bundled-${platform}-`;
  const artifactName = basename(artifactRoot);
  if (
    !artifactName.startsWith(expectedPrefix) ||
    !/^[0-9a-f]{64}$/.test(artifactName.slice(expectedPrefix.length))
  ) {
    fail('artifact name is not hash-addressed');
  }
  const entries = readdirSync(artifactRoot);
  if (entries.length !== 1 || entries[0] !== 'credentialed-output.tar') {
    fail('handoff contains an unexpected file');
  }
  const archivePath = join(artifactRoot, entries[0]);
  const metadata = lstatSync(archivePath);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    fail('handoff is not an exact regular file');
  }
  assertCredentialedArchiveSize(metadata.size);
  const archive = readFileSync(archivePath);
  const actual = createHash('sha256').update(archive).digest('hex');
  if (actual !== artifactName.slice(expectedPrefix.length)) {
    fail('bytes do not match the artifact hash address');
  }
  return archive;
}

function main() {
  const [platform, downloadRoot, outputRootInput] = process.argv.slice(2);
  if (platform === undefined || downloadRoot === undefined || outputRootInput === undefined) {
    throw new Error(
      'usage: extract-credentialed-output.mjs <platform> <download-root> <output-root>',
    );
  }
  const outputRoot = resolve(outputRootInput);
  if (basename(outputRoot) !== 'mdreader-credentialed-output') {
    fail('extraction root is not the dedicated runner path');
  }
  const archive = verifyHandoff(platform, downloadRoot);
  const members = parseArchive(archive);
  const namespace = closeNamespace(members);
  materialize(members, namespace, outputRoot);
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main();
}
