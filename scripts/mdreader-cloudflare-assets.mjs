import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readSync,
  realpathSync,
  readdirSync,
  renameSync,
  rmSync,
  writeSync,
} from 'node:fs';
import { platform } from 'node:os';
import {
  basename,
  dirname,
  isAbsolute,
  join,
  parse,
  relative,
  resolve,
  sep,
} from 'node:path';

const MIB = 1024 * 1024;

export const MDREADER_CLOUDFLARE_ASSET_LIMITS = Object.freeze({
  maxFiles: 20_000,
  maxFileBytes: 25 * MIB,
  maxTotalBytes: 64 * MIB,
});

function displayBytes(value) {
  return value.toLocaleString('en-US');
}

function safeRelativePath(path, name) {
  assert.equal(typeof path, 'string', `${name} must be a relative path`);
  assert.equal(isAbsolute(path), false, `${name} must be a relative path`);
  assert.equal(path.includes('\\'), false, `${name} must use forward slashes`);
  assert.equal(path.includes('\0'), false, `${name} contains a null byte`);
  const segments = path.split('/');
  assert.ok(
    segments.length > 0
      && segments.every((segment) => segment !== '' && segment !== '.' && segment !== '..'),
    `${name} contains an unsafe path segment: ${path}`,
  );
  return segments;
}

function snapshotIdentity(metadata, path, kind, name) {
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset source rejects symbolic link ${name}`,
  );
  assert.equal(
    kind === 'file' ? metadata.isFile() : metadata.isDirectory(),
    true,
    `Cloudflare asset source is not a regular ${kind}: ${name}`,
  );
  assert.ok(
    metadata.dev !== 0n && metadata.ino !== 0n,
    `Cloudflare asset source cannot establish stable identity for ${name} at ${path}`,
  );
  return Object.freeze({
    ctimeNs: metadata.ctimeNs,
    dev: metadata.dev,
    ino: metadata.ino,
    kind,
    mode: metadata.mode,
    mtimeNs: metadata.mtimeNs,
    size: metadata.size,
  });
}

function lstatBigInt(path, name) {
  try {
    return lstatSync(path, { bigint: true });
  } catch (error) {
    throw new Error(
      `Cloudflare asset source identity changed or disappeared for ${name}: ${error.message}`,
      { cause: error },
    );
  }
}

function snapshotDirectory(path, name, canonical = false) {
  const absolutePath = resolve(path);
  const identity = snapshotIdentity(
    lstatBigInt(absolutePath, name),
    absolutePath,
    'directory',
    name,
  );
  if (canonical) {
    assert.equal(
      realpathSync.native(absolutePath),
      absolutePath,
      `Cloudflare asset source root is not canonical: ${name}`,
    );
  }
  return Object.freeze({ identity, name, path: absolutePath });
}

function snapshotCanonicalDirectoryChain(path, name) {
  const absolutePath = resolve(path);
  const requestedRoot = snapshotDirectory(absolutePath, name);
  assert.equal(
    realpathSync.native(absolutePath),
    absolutePath,
    `Cloudflare asset source root is not canonical: ${name}`,
  );
  const root = parse(absolutePath).root;
  const segments = relative(root, absolutePath).split(sep).filter(Boolean);
  const ancestors = [
    absolutePath === root
      ? requestedRoot
      : snapshotDirectory(root, `${name} filesystem root`),
  ];
  let current = root;
  for (const segment of segments) {
    current = join(current, segment);
    ancestors.push(
      current === absolutePath
        ? requestedRoot
        : snapshotDirectory(current, `${name} ancestor ${segment}`),
    );
  }
  return Object.freeze(ancestors);
}

function assertSameIdentity(metadata, expected, name, role = 'source') {
  const actualKind = metadata.isFile()
    ? 'file'
    : metadata.isDirectory()
      ? 'directory'
      : 'other';
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset ${role} rejects symbolic link ${name}`,
  );
  assert.equal(
    actualKind,
    expected.kind,
    `Cloudflare asset ${role} identity changed for ${name}`,
  );
  const fields = expected.kind === 'file'
    ? ['dev', 'ino', 'mode', 'size', 'mtimeNs', 'ctimeNs']
    : ['dev', 'ino', 'mode'];
  for (const field of fields) {
    assert.equal(
      metadata[field],
      expected[field],
      `Cloudflare asset ${role} identity changed for ${name} (${field})`,
    );
  }
}

function assertCurrentDirectory(directory, role = 'source ancestor') {
  assertSameIdentity(
    lstatBigInt(directory.path, directory.name),
    directory.identity,
    directory.name,
    role,
  );
}

function snapshotFile(root, relativePath, sourceName, rootAncestors = [root]) {
  const segments = safeRelativePath(relativePath, sourceName);
  const ancestors = [...rootAncestors];
  let current = root.path;
  for (const segment of segments.slice(0, -1)) {
    current = join(current, segment);
    ancestors.push(snapshotDirectory(current, `${sourceName} ancestor ${segment}`));
  }
  const path = join(current, segments.at(-1));
  const identity = snapshotIdentity(
    lstatBigInt(path, sourceName),
    path,
    'file',
    sourceName,
  );
  return Object.freeze({
    ancestors: Object.freeze(ancestors),
    identity,
    path,
    relativePath,
    sourceName,
  });
}

/**
 * Captures one source file and every intended directory ancestor beneath a
 * canonical root. The returned identity is rechecked at the effect boundary.
 */
export function snapshotMdreaderCloudflareSourceFile(
  rootPath,
  relativePath,
  sourceName,
) {
  const rootAncestors = snapshotCanonicalDirectoryChain(
    rootPath,
    `${sourceName} root`,
  );
  const root = rootAncestors.at(-1);
  return snapshotFile(root, relativePath, sourceName, rootAncestors);
}

/**
 * Captures every regular file in a source tree without following links.
 */
export function snapshotMdreaderCloudflareSourceTree(rootPath, sourceName) {
  const rootAncestors = snapshotCanonicalDirectoryChain(rootPath, sourceName);
  const root = rootAncestors.at(-1);
  const files = [];

  function walk(current, relativeDirectory, ancestors) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const relativePath = relativeDirectory
        ? `${relativeDirectory}/${entry}`
        : entry;
      const name = `${sourceName}/${relativePath}`;
      const metadata = lstatBigInt(path, name);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare asset source rejects symbolic link ${name}`,
      );
      if (metadata.isDirectory()) {
        const directory = Object.freeze({
          identity: snapshotIdentity(metadata, path, 'directory', name),
          name,
          path,
        });
        walk(path, relativePath, [...ancestors, directory]);
        continue;
      }
      const identity = snapshotIdentity(metadata, path, 'file', name);
      files.push(Object.freeze({
        ancestors: Object.freeze(ancestors),
        identity,
        path,
        relativePath,
        sourceName: name,
      }));
    }
  }

  walk(root.path, '', rootAncestors);
  return Object.freeze(files);
}

/**
 * Fails explicitly if the host cannot guarantee no-follow source opens.
 */
export function assertMdreaderPinnedCopySupport() {
  assert.ok(
    Number.isInteger(constants.O_NOFOLLOW) && constants.O_NOFOLLOW !== 0,
    `Safe pinned Cloudflare asset staging is unavailable on ${platform()}: O_NOFOLLOW is required`,
  );
  return constants.O_NOFOLLOW;
}

function invokeSourceOpenSeam(source, destination) {
  const seam = globalThis[Symbol.for('mdreader.cloudflare.before-source-open')];
  if (seam === undefined) return;
  assert.equal(
    typeof seam,
    'function',
    'Cloudflare source-open seam must be a function',
  );
  const result = seam({
    destination,
    source: source.path,
    sourceName: source.sourceName,
  });
  assert.equal(
    result instanceof Promise,
    false,
    'Cloudflare source-open seam must be synchronous',
  );
}

function consumePinnedSource(
  source,
  { destination = undefined, invokeSeam = false, onChunk, onReady },
) {
  const noFollow = assertMdreaderPinnedCopySupport();
  if (invokeSeam) invokeSourceOpenSeam(source, destination);
  for (const ancestor of source.ancestors) assertCurrentDirectory(ancestor);

  let descriptor;
  try {
    descriptor = openSync(source.path, constants.O_RDONLY | noFollow);
  } catch (error) {
    throw new Error(
      `Cloudflare asset source no-follow open failed for ${source.sourceName}: ${error.message}`,
      { cause: error },
    );
  }

  try {
    const before = fstatSync(descriptor, { bigint: true });
    assertSameIdentity(before, source.identity, source.sourceName);
    onReady?.();

    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytes = 0;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      const chunk = buffer.subarray(0, count);
      hash.update(chunk);
      onChunk(chunk);
      bytes += count;
    }

    const after = fstatSync(descriptor, { bigint: true });
    assertSameIdentity(after, source.identity, source.sourceName);
    assert.equal(
      BigInt(bytes),
      source.identity.size,
      `Cloudflare asset source changed while reading ${source.sourceName}`,
    );
    return { bytes, digest: hash.digest('hex') };
  } finally {
    closeSync(descriptor);
  }
}

/**
 * Reads a snapshotted metadata file through its already-verified descriptor.
 */
export function readPinnedMdreaderCloudflareSource(source) {
  const chunks = [];
  const result = consumePinnedSource(source, {
    onChunk(chunk) {
      chunks.push(Buffer.from(chunk));
    },
  });
  return Buffer.concat(chunks, result.bytes);
}

function assertDestinationPlan(plan) {
  assert.ok(Array.isArray(plan), 'Cloudflare destination plan must be an array');
  const sorted = [...plan].sort((left, right) =>
    left.destination < right.destination
      ? -1
      : left.destination > right.destination
        ? 1
        : 0);
  const destinations = new Set();
  let totalBytes = 0n;
  for (let index = 0; index < sorted.length; index += 1) {
    const entry = sorted[index];
    safeRelativePath(entry.destination, 'Cloudflare asset destination');
    assert.ok(entry.source?.identity, `Missing source snapshot for ${entry.destination}`);
    assert.ok(
      index < MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles,
      `Cloudflare asset staging contains more than ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles)} files`,
    );
    assert.ok(
      entry.source.identity.size <= BigInt(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes),
      `Cloudflare asset ${entry.destination} is ${displayBytes(Number(entry.source.identity.size))} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes)}`,
    );
    totalBytes += entry.source.identity.size;
    assert.ok(
      totalBytes <= BigInt(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes),
      `Cloudflare asset staging total exceeds ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes)} bytes`,
    );

    assert.equal(
      destinations.has(entry.destination),
      false,
      `Cloudflare asset destination ${entry.destination} collides with another source`,
    );
    let slash = entry.destination.indexOf('/');
    while (slash !== -1) {
      const ancestor = entry.destination.slice(0, slash);
      assert.ok(
        !destinations.has(ancestor),
        `Cloudflare asset destination ${entry.destination} collides with ${ancestor}`,
      );
      slash = entry.destination.indexOf('/', slash + 1);
    }
    destinations.add(entry.destination);
  }
  return sorted;
}

/**
 * Validates exact and file/directory-prefix collisions before staging writes.
 */
export function assertMdreaderCloudflareDestinationPlan(plan) {
  return assertDestinationPlan(plan);
}

function writeAll(descriptor, chunk) {
  let offset = 0;
  while (offset < chunk.length) {
    const written = writeSync(
      descriptor,
      chunk,
      offset,
      chunk.length - offset,
    );
    assert.ok(written > 0, 'Cloudflare staged asset write made no progress');
    offset += written;
  }
}

function ensureDestinationDirectory(root, relativeDirectory, rootSnapshot) {
  assertCurrentDirectory(rootSnapshot, 'destination ancestor');
  const ancestors = [rootSnapshot];
  let current = root;
  if (relativeDirectory === '.') return ancestors;
  for (const segment of safeRelativePath(
    relativeDirectory.split(sep).join('/'),
    'Cloudflare destination directory',
  )) {
    current = join(current, segment);
    try {
      mkdirSync(current, { mode: 0o700 });
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
    }
    const metadata = lstatSync(current, { bigint: true });
    assert.equal(
      metadata.isSymbolicLink(),
      false,
      `Cloudflare asset destination rejects symbolic link ${current}`,
    );
    assert.equal(
      metadata.isDirectory(),
      true,
      `Cloudflare asset destination ancestor is not a directory: ${current}`,
    );
    ancestors.push(Object.freeze({
      identity: snapshotIdentity(
        metadata,
        current,
        'directory',
        `destination ancestor ${current}`,
      ),
      name: `destination ancestor ${current}`,
      path: current,
    }));
  }
  return ancestors;
}

function digestDestination(path, expectedBytes, ancestors) {
  for (const ancestor of ancestors) {
    assertCurrentDirectory(ancestor, 'destination ancestor');
  }
  const descriptor = openSync(
    path,
    constants.O_RDONLY | assertMdreaderPinnedCopySupport(),
  );
  try {
    const metadata = fstatSync(descriptor, { bigint: true });
    assert.equal(
      metadata.isFile(),
      true,
      `Cloudflare staged asset is not a regular file: ${path}`,
    );
    assert.equal(
      metadata.size,
      BigInt(expectedBytes),
      `Cloudflare staged asset size changed: ${path}`,
    );
    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytes = 0;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
      bytes += count;
    }
    assert.equal(bytes, expectedBytes, `Cloudflare staged asset changed: ${path}`);
    return hash.digest('hex');
  } finally {
    closeSync(descriptor);
  }
}

function copyPinnedSource(source, destination, destinationAncestors) {
  let destinationDescriptor;
  try {
    const result = consumePinnedSource(source, {
      destination,
      invokeSeam: true,
      onReady() {
        for (const ancestor of destinationAncestors) {
          assertCurrentDirectory(ancestor, 'destination ancestor');
        }
        destinationDescriptor = openSync(
          destination,
          constants.O_WRONLY
            | constants.O_CREAT
            | constants.O_EXCL
            | assertMdreaderPinnedCopySupport(),
          0o600,
        );
      },
      onChunk(chunk) {
        writeAll(destinationDescriptor, chunk);
      },
    });
    closeSync(destinationDescriptor);
    destinationDescriptor = undefined;
    assert.equal(
      digestDestination(destination, result.bytes, destinationAncestors),
      result.digest,
      `Cloudflare staged asset digest differs from pinned source ${source.sourceName}`,
    );
  } finally {
    if (destinationDescriptor !== undefined) closeSync(destinationDescriptor);
  }
}

function stagedFilePaths(directory) {
  const paths = [];
  function walk(current) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const metadata = lstatSync(path);
      if (metadata.isDirectory()) {
        walk(path);
      } else if (metadata.isFile()) {
        paths.push(relative(directory, path).split(sep).join('/'));
      }
    }
  }
  walk(directory);
  return paths.sort();
}

/**
 * Copies a validated plan into a fresh private tree and atomically publishes
 * the completed asset set beneath the scoped .open-next directory.
 */
export function stageMdreaderCloudflareAssetPlan(plan, openNext, assets) {
  const sorted = assertDestinationPlan(plan);
  assertMdreaderPinnedCopySupport();
  const absoluteOpenNext = resolve(openNext);
  const absoluteAssets = resolve(assets);
  assert.equal(
    dirname(absoluteAssets),
    absoluteOpenNext,
    'Cloudflare assets must be a direct child of the scoped .open-next directory',
  );
  assert.equal(
    basename(absoluteOpenNext),
    '.open-next',
    'Cloudflare staging may replace only the scoped .open-next directory',
  );

  try {
    const existing = lstatSync(absoluteOpenNext);
    assert.equal(
      existing.isSymbolicLink(),
      false,
      `Cloudflare asset destination rejects symbolic link ${absoluteOpenNext}`,
    );
    assert.equal(
      existing.isDirectory(),
      true,
      `Cloudflare asset destination is not a directory: ${absoluteOpenNext}`,
    );
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  rmSync(absoluteOpenNext, { recursive: true, force: true });
  mkdirSync(absoluteOpenNext, { mode: 0o700 });
  const openNextSnapshot = snapshotDirectory(
    absoluteOpenNext,
    'Cloudflare .open-next staging root',
    true,
  );
  let temporary = mkdtempSync(join(absoluteOpenNext, '.assets-stage-'));
  const temporarySnapshot = snapshotDirectory(
    temporary,
    'Cloudflare private asset staging root',
    true,
  );
  let published = false;

  try {
    for (const entry of sorted) {
      assertCurrentDirectory(openNextSnapshot);
      const destinationAncestors = ensureDestinationDirectory(
        temporary,
        dirname(entry.destination),
        temporarySnapshot,
      );
      copyPinnedSource(
        entry.source,
        join(temporary, entry.destination),
        destinationAncestors,
      );
    }
    const staged = assertMdreaderCloudflareAssets(temporary);
    assert.deepEqual(
      stagedFilePaths(temporary),
      sorted.map((entry) => entry.destination),
      'Cloudflare private staging tree differs from the validated copy plan',
    );
    assertCurrentDirectory(openNextSnapshot);
    assertCurrentDirectory(temporarySnapshot);
    try {
      lstatSync(absoluteAssets);
      assert.fail(`Cloudflare asset publish destination already exists: ${absoluteAssets}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    renameSync(temporary, absoluteAssets);
    temporary = undefined;
    const result = assertMdreaderCloudflareAssets(absoluteAssets);
    assert.deepEqual(result, staged);
    published = true;
    return result;
  } finally {
    if (temporary !== undefined) rmSync(temporary, { recursive: true, force: true });
    if (!published) rmSync(absoluteAssets, { recursive: true, force: true });
  }
}

export function assertMdreaderCloudflareAssets(directory) {
  const root = lstatSync(directory);
  assert.equal(
    root.isSymbolicLink(),
    false,
    `Cloudflare asset staging rejects symbolic link ${directory}`,
  );
  assert.equal(
    root.isDirectory(),
    true,
    `Cloudflare asset staging root is not a directory: ${directory}`,
  );

  let fileCount = 0;
  let totalBytes = 0;
  let largestFileBytes = 0;
  let largestFile = '';

  function walk(current) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const stagedPath = relative(directory, path).split(sep).join('/');
      const metadata = lstatSync(path);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare asset staging rejects symbolic link ${stagedPath}`,
      );
      if (metadata.isDirectory()) {
        walk(path);
        continue;
      }
      assert.equal(
        metadata.isFile(),
        true,
        `Cloudflare asset staging rejects non-regular entry ${stagedPath}`,
      );

      fileCount += 1;
      assert.ok(
        fileCount <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles,
        `Cloudflare asset staging contains ${displayBytes(fileCount)} files; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles)}`,
      );
      assert.ok(
        metadata.size <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes,
        `Cloudflare asset ${stagedPath} is ${displayBytes(metadata.size)} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes)}`,
      );

      totalBytes += metadata.size;
      assert.ok(
        totalBytes <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes,
        `Cloudflare asset staging total is ${displayBytes(totalBytes)} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes)}`,
      );
      if (metadata.size > largestFileBytes) {
        largestFileBytes = metadata.size;
        largestFile = stagedPath;
      }
    }
  }

  walk(directory);
  return { fileCount, totalBytes, largestFile, largestFileBytes };
}
